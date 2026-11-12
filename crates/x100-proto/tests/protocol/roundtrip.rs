//! Every request, response and notification survives its wire form, and bad lines fail with typed
//! errors.

use serde_json::{Value, json};
use x100_core::event::Envelope;
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::money::Usage;
use x100_core::terminal::{Chunk, ChunkKind, TermPos};
use x100_core::testing::{daemon_log, fernleaf};
use x100_core::transcript::{Entry, Item, Pos, TurnEnd};
use x100_proto::*;

use crate::samples::{at, id, samples};

#[test]
fn every_method_has_a_sample_and_every_sample_a_method() {
    let sampled: Vec<&str> = samples().iter().map(|s| s.name).collect();
    let listed: Vec<&str> = METHODS.iter().map(|m| m.name).collect();
    assert_eq!(sampled, listed);
    let mut unique = listed.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), listed.len(), "a method name is used twice");
}

#[test]
fn every_error_code_has_its_own_number_and_name_and_comes_back_from_the_wire() {
    let mut numbers = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for code in ErrorCode::ALL {
        assert!(numbers.insert(code.number()));
        assert!(names.insert(code.name()));
        assert_eq!(ErrorCode::from_number(code.number()), Some(code));
        let reply = Message::Response(Response::err(
            Some(Id::Text("a".into())),
            RpcError::new(code, "because"),
        ));
        assert_eq!(Message::parse(&reply.to_line()), Ok(reply));
    }
    // JSON-RPC's own five keep their reserved numbers.
    assert_eq!(ErrorCode::MethodNotFound.number(), -32601);
}

#[test]
fn a_response_with_an_error_code_this_version_does_not_know_is_refused_not_guessed() {
    let line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"x"}}"#;
    assert_eq!(
        Message::parse(line).unwrap_err().error.code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn whole_states_round_trip_as_results() {
    let project = ProjectState::fold(&fernleaf().log).unwrap();
    let daemon = DaemonState::fold(&daemon_log(at(0))).unwrap();
    let get_project = Response::ok::<GetProject>(Id::Number(1), &project);
    let get_daemon = Response::ok::<GetDaemon>(Id::Number(2), &daemon);
    for (response, check) in [(get_project, true), (get_daemon, false)] {
        let line = Message::Response(response.clone()).to_line();
        let Ok(Message::Response(back)) = Message::parse(&line) else {
            panic!("did not parse");
        };
        assert_eq!(back, response);
        if check {
            assert_eq!(back.into_result::<GetProject>().unwrap(), project);
        } else {
            assert_eq!(back.into_result::<GetDaemon>().unwrap(), daemon);
        }
    }
}

#[test]
fn trials_with_real_runs_round_trip() {
    let project = ProjectState::fold(&fernleaf().log).unwrap();
    let trials: Vec<_> = project.trials().cloned().collect();
    assert!(!trials.is_empty());
    let result = Trials { trials };
    let r = Response::ok::<ListTrials>(Id::Number(1), &result);
    let Ok(Message::Response(back)) = Message::parse(&Message::Response(r).to_line()) else {
        panic!()
    };
    assert_eq!(back.into_result::<ListTrials>().unwrap(), result);
}

fn through_a_line(notice: Notice) -> Notice {
    let line = Message::Notice(notice).to_line();
    let top: Value = serde_json::from_str(&line).unwrap();
    assert!(top.get("id").is_none(), "a notification has no id");
    match Message::parse(&line) {
        Ok(Message::Notice(n)) => n,
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_notification_round_trips() {
    let sub = SubscriptionId(3);
    let project = id(x100_core::id::ProjectId::new, 1);
    let project_event = fernleaf().log.remove(0);
    let daemon_event = daemon_log(at(0)).remove(0);
    let items = vec![
        Item::TurnStarted,
        Item::Text {
            text: "hi\nthere".into(),
        },
        Item::ToolCall {
            call: "c1".into(),
            name: "bash".into(),
            input: json!({"command": "ls -la"}),
        },
        Item::ToolOutput {
            call: "c1".into(),
            chunk: "total 0\r\n".into(),
        },
        Item::ToolResult {
            call: "c1".into(),
            output: "total 0\n".into(),
            is_error: false,
        },
        Item::Usage {
            usage: Usage {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write_short: 4,
                cache_write_long: 5,
            },
        },
        Item::TurnEnded {
            how: TurnEnd::Completed,
        },
        Item::TurnEnded {
            how: TurnEnd::Failed {
                detail: "boom".into(),
            },
        },
        Item::TurnEnded {
            how: TurnEnd::Interrupted,
        },
        Item::DriverChanged { from: None },
        Item::DriverChanged {
            from: Some(id(x100_core::id::DriverId::new, 2)),
        },
        Item::StartPack {
            text: "ROLE: Tech\nYour id: agt_2\n".into(),
        },
    ];
    let mut notices = vec![
        Notice::Event(EventNotice::Daemon {
            subscription: sub,
            envelope: daemon_event,
        }),
        Notice::Event(EventNotice::Project {
            subscription: sub,
            project,
            envelope: project_event,
        }),
        Notice::Closed(SubscriptionClosed {
            subscription: sub,
            because: ClosedBecause::Lagged,
        }),
        Notice::Closed(SubscriptionClosed {
            subscription: sub,
            because: ClosedBecause::Shutdown,
        }),
    ];
    for (i, item) in items.into_iter().enumerate() {
        notices.push(Notice::TranscriptEntry(TranscriptNotice {
            subscription: sub,
            position: Pos(i as u64 + 1),
            entry: Entry {
                at: at(i as u64),
                driver: id(x100_core::id::DriverId::new, 2),
                item,
            },
        }));
    }
    let all: Vec<u8> = (0..=255).collect();
    let chunks = [
        ChunkKind::Resize { cols: 80, rows: 24 },
        ChunkKind::Output { bytes: Bytes(all) },
    ];
    for (i, what) in chunks.into_iter().enumerate() {
        notices.push(Notice::TerminalChunk(TerminalNotice {
            subscription: sub,
            position: TermPos(i as u64 + 1),
            chunk: Chunk {
                at: at(i as u64),
                what,
            },
        }));
    }
    for n in notices {
        assert_eq!(through_a_line(n.clone()), n);
    }
    // The table and the type agree on the names.
    let names: Vec<&str> = NOTICES.iter().map(|n| n.0).collect();
    assert_eq!(
        names,
        [
            "events.event",
            "transcript.entry",
            "terminal.chunk",
            "subscription.closed"
        ]
    );
}

#[test]
fn both_log_pages_and_both_orders_round_trip() {
    let daemon = daemon_log(at(0));
    let pages = [
        LogPage::Daemon { events: daemon },
        LogPage::Project {
            events: fernleaf().log[..5].to_vec(),
        },
    ];
    for p in pages {
        let back: LogPage = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }
    let reads = [
        ReadLog {
            log: LogId::Daemon,
            page: Page::OldestFirst {
                after: Some(x100_core::event::Seq(4)),
                limit: 50,
            },
        },
        ReadLog {
            log: LogId::Daemon,
            page: Page::NewestFirst {
                before: None,
                limit: 50,
            },
        },
    ];
    for r in reads {
        let back: ReadLog = serde_json::from_value(serde_json::to_value(r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}

#[test]
fn a_cursor_names_where_each_log_got_to() {
    let log = fernleaf().log;
    let state = ProjectState::fold(&log[..10]).unwrap();
    let project = id(x100_core::id::ProjectId::new, 1);
    let snapshot = Snapshot {
        daemon: DaemonState::fold(&daemon_log(at(0))[..3]).unwrap(),
        projects: [(project, state)].into(),
    };
    let cursor = snapshot.cursor();
    assert_eq!(cursor.daemon, Some(x100_core::event::Seq(3)));
    assert_eq!(cursor.projects[&project], x100_core::event::Seq(10));
    let back: Cursor = serde_json::from_value(serde_json::to_value(&cursor).unwrap()).unwrap();
    assert_eq!(back, cursor);
}

/// What a client does with the protocol: take the snapshot, apply the notified events. Wherever
/// the snapshot was taken, it ends in the state the whole log gives.
#[test]
fn a_snapshot_plus_the_notified_events_is_the_state_of_the_whole_log() {
    let log = fernleaf().log;
    let whole = ProjectState::fold(&log).unwrap();
    let project = id(x100_core::id::ProjectId::new, 1);
    for split in [0, 1, 2, 77, 400, log.len()] {
        let snapshot = Snapshot {
            daemon: DaemonState::default(),
            projects: if split == 0 {
                Default::default()
            } else {
                [(project, ProjectState::fold(&log[..split]).unwrap())].into()
            },
        };
        let Value::Object(_) = serde_json::to_value(&snapshot).unwrap() else {
            panic!()
        };
        let mut client: Snapshot =
            serde_json::from_str(&serde_json::to_string(&snapshot).unwrap()).unwrap();
        for envelope in &log[split..] {
            let n = Notice::Event(EventNotice::Project {
                subscription: SubscriptionId(1),
                project,
                envelope: envelope.clone(),
            });
            let Notice::Event(EventNotice::Project {
                project, envelope, ..
            }) = through_a_line(n)
            else {
                panic!()
            };
            client
                .projects
                .entry(project)
                .or_default()
                .apply(&envelope)
                .unwrap();
        }
        assert_eq!(client.projects[&project], whole, "split at {split}");
    }
}

#[test]
fn a_bad_line_gets_a_typed_error_and_keeps_the_request_id_when_it_has_one() {
    let code = |line: &str| {
        let e = Message::parse(line).unwrap_err();
        (e.error.code, e.id)
    };
    assert_eq!(code("not json"), (ErrorCode::ParseError, None));
    assert_eq!(code("[]"), (ErrorCode::InvalidRequest, None));
    assert_eq!(
        code(r#"{"id":1,"method":"hello"}"#),
        (ErrorCode::InvalidRequest, None)
    );
    assert_eq!(
        code(r#"{"jsonrpc":"1.0","id":1,"method":"hello"}"#),
        (ErrorCode::InvalidRequest, Some(Id::Number(1)))
    );
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":"q","method":"no.such","params":{}}"#),
        (ErrorCode::MethodNotFound, Some(Id::Text("q".into())))
    );
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":2,"method":"hello","params":{"client":1}}"#),
        (ErrorCode::InvalidParams, Some(Id::Number(2)))
    );
    // A request with neither method nor result.
    assert_eq!(
        code(r#"{"jsonrpc":"2.0","id":3}"#),
        (ErrorCode::InvalidRequest, Some(Id::Number(3)))
    );
}

#[test]
fn an_id_of_the_wrong_kind_is_refused_at_the_boundary() {
    let task = id(x100_core::id::TaskId::new, 1);
    let line = json!({"jsonrpc":"2.0","id":1,"method":"projects.get","params":{"project": task.to_string()}});
    let e = Message::parse(&line.to_string()).unwrap_err();
    assert_eq!(e.error.code, ErrorCode::InvalidParams);
    assert_eq!(e.id, Some(Id::Number(1)));
}

#[test]
fn bytes_travel_as_base64_and_a_bad_encoding_is_an_invalid_param() {
    let params = json!({"project": id(x100_core::id::ProjectId::new, 1).to_string(),
        "name":"a","media_type":"text/plain","data":"aGVsbG8="});
    let Ok(Call::UploadFile(u)) = Call::parse("files.upload", params.clone()) else {
        panic!()
    };
    assert_eq!(u.data, Bytes(b"hello".to_vec()));
    let mut bad = params;
    bad["data"] = json!("not base64!!");
    assert_eq!(
        Call::parse("files.upload", bad).unwrap_err().code,
        ErrorCode::InvalidParams
    );

    // A large file is still one line.
    let big = UploadFile {
        data: Bytes(vec![7; 3_000_000]),
        ..u
    };
    let line = Message::Request(Request {
        id: Id::Number(1),
        call: big.clone().into(),
    })
    .to_line();
    assert!(!line.contains('\n'));
    assert_eq!(
        Message::parse(&line),
        Ok(Message::Request(Request {
            id: Id::Number(1),
            call: big.into()
        }))
    );
}

#[test]
fn a_credential_value_is_redacted_everywhere_but_the_one_wire_field() {
    let call = Call::from(AddCredential {
        name: "k".into(),
        value: Secret::new("sk_live_hunter2"),
        scope: crate::samples::scope(),
    });
    assert!(!format!("{call:?}").contains("hunter2"));
    assert_eq!(call.params()["value"], "sk_live_hunter2");
    // No result of any method contains it: the types that carry results cannot hold a Secret.
    for s in samples() {
        assert!(!s.result.to_string().contains("hunter2"));
    }
    // The same for a decision model's key.
    let call = Call::from(ConnectDecisionModel {
        provider: x100_core::decision::DecisionProvider::Gemini,
        key: Secret::new("AIza_hunter3"),
    });
    assert!(!format!("{call:?}").contains("hunter3"));
    assert_eq!(call.params()["key"], "AIza_hunter3");
}

#[test]
fn ids_in_results_keep_their_kind_prefix() {
    let s = samples();
    let created = s.iter().find(|s| s.name == "projects.create").unwrap();
    assert!(
        created.result["project"]
            .as_str()
            .unwrap()
            .starts_with("prj_")
    );
}

#[test]
fn envelopes_keep_their_shape() {
    let e: Envelope<x100_core::event::ProjectEvent> = fernleaf().log.remove(0);
    let v = serde_json::to_value(&e).unwrap();
    for key in ["seq", "at", "by", "event"] {
        assert!(v.get(key).is_some(), "{key}");
    }
}
