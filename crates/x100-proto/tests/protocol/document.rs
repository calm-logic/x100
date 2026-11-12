//! `PROTOCOL.md` and the method table say the same thing, and what the document shows is accepted
//! by the types.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde_json::Value;
use x100_core::transcript::{self, Item, Pos};
use x100_proto::*;

use crate::samples::{id, samples};

const DOCUMENT: &str = include_str!("../../PROTOCOL.md");

#[derive(Default)]
struct Entry {
    errors: Option<Vec<String>>,
    params: Option<Value>,
    result: Option<Value>,
    notice: Option<Value>,
}

struct Parsed {
    methods: BTreeMap<String, Entry>,
    notices: BTreeMap<String, Entry>,
    /// The worked example's lines, tagged `client` or `daemon`, in order.
    exchange: Vec<(String, String)>,
}

fn parse(doc: &str) -> Parsed {
    let mut methods: BTreeMap<String, Entry> = BTreeMap::new();
    let mut notices: BTreeMap<String, Entry> = BTreeMap::new();
    let mut exchange = Vec::new();
    let (mut section, mut current) = (String::new(), None::<String>);
    let mut lines = doc.lines();
    while let Some(line) = lines.next() {
        if let Some(h) = line.strip_prefix("## ") {
            section = h.trim().to_owned();
            current = None;
        } else if let Some(h) = line.strip_prefix("### ") {
            let name = h.trim().trim_matches('`').to_owned();
            match section.as_str() {
                "Methods" => drop(methods.entry(name.clone()).or_default()),
                "Notifications" => drop(notices.entry(name.clone()).or_default()),
                _ => {}
            }
            current = Some(name);
        } else if let Some(list) = line.strip_prefix("Errors:") {
            let entry = methods
                .get_mut(current.as_deref().expect("Errors: under a method"))
                .expect("a method");
            let list = list.trim().trim_end_matches('.');
            entry.errors = Some(if list == "none" {
                vec![]
            } else {
                list.split(',').map(|e| e.trim().to_owned()).collect()
            });
        } else if let Some(info) = line.strip_prefix("```json ") {
            let kind = info.trim();
            let mut body = String::new();
            for l in lines.by_ref() {
                if l.starts_with("```") {
                    break;
                }
                body.push_str(l);
                body.push('\n');
            }
            if section == "Worked example" {
                for l in body.lines() {
                    exchange.push((kind.to_owned(), l.to_owned()));
                }
                continue;
            }
            let value: Value = serde_json::from_str(&body)
                .unwrap_or_else(|e| panic!("{current:?} {kind}: not JSON: {e}"));
            let name = current.clone().expect("a block under a heading");
            match (section.as_str(), kind) {
                ("Methods", "params") => methods.get_mut(&name).unwrap().params = Some(value),
                ("Methods", "result") => methods.get_mut(&name).unwrap().result = Some(value),
                ("Notifications", "notice") => notices.get_mut(&name).unwrap().notice = Some(value),
                other => panic!(
                    "a `json {}` block in section {:?} under {name}",
                    other.1, other.0
                ),
            }
        }
    }
    Parsed {
        methods,
        notices,
        exchange,
    }
}

#[test]
fn the_document_lists_exactly_the_methods_the_types_have() {
    let doc = parse(DOCUMENT);
    let in_types: Vec<&str> = METHODS.iter().map(|m| m.name).collect();
    let in_doc: Vec<&str> = doc.methods.keys().map(String::as_str).collect();
    let missing: Vec<_> = in_types.iter().filter(|m| !in_doc.contains(m)).collect();
    let extra: Vec<_> = in_doc.iter().filter(|m| !in_types.contains(m)).collect();
    assert!(
        missing.is_empty(),
        "in the types but not in PROTOCOL.md: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "in PROTOCOL.md but not in the types: {extra:?}"
    );
    // And in the same order, so the document reads as the table does.
    let order: Vec<&str> = DOCUMENT
        .lines()
        .filter_map(|l| l.strip_prefix("### `"))
        .map(|l| l.trim_end_matches('`'))
        .filter(|n| in_types.contains(n))
        .collect();
    assert_eq!(order, in_types);
}

#[test]
fn the_document_lists_exactly_the_notifications_the_types_have() {
    let doc = parse(DOCUMENT);
    let mut in_types: Vec<&str> = NOTICES.iter().map(|n| n.0).collect();
    let mut in_doc: Vec<&str> = doc.notices.keys().map(String::as_str).collect();
    in_types.sort_unstable();
    in_doc.sort_unstable();
    assert_eq!(in_doc, in_types);
    for (name, entry) in &doc.notices {
        let v = entry
            .notice
            .clone()
            .unwrap_or_else(|| panic!("{name} has no example"));
        let notice = Notice::parse(name, v.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            notice.params(),
            v,
            "{name}: the example drops or invents a field"
        );
    }
}

#[test]
fn each_method_lists_the_errors_the_types_say_it_returns() {
    let doc = parse(DOCUMENT);
    for m in METHODS {
        let entry = &doc.methods[m.name];
        let mut documented = entry
            .errors
            .clone()
            .unwrap_or_else(|| panic!("{} has no `Errors:` line", m.name));
        let mut real: Vec<String> = m.errors.iter().map(|e| e.name().to_owned()).collect();
        documented.sort();
        real.sort();
        assert_eq!(documented, real, "{}", m.name);
    }
}

#[test]
fn each_example_in_the_document_is_accepted_by_the_types_and_loses_nothing() {
    let doc = parse(DOCUMENT);
    for m in METHODS {
        let entry = &doc.methods[m.name];
        let params = entry
            .params
            .clone()
            .unwrap_or_else(|| panic!("{} has no params example", m.name));
        let call = Call::parse(m.name, params.clone())
            .unwrap_or_else(|e| panic!("{} params: {e}", m.name));
        assert_eq!(
            call.params(),
            params,
            "{}: the params example drops or invents a field",
            m.name
        );
        let result = entry
            .result
            .clone()
            .unwrap_or_else(|| panic!("{} has no result example", m.name));
        let again = reserialize_result(m.name, result.clone())
            .unwrap_or_else(|e| panic!("{} result: {e}", m.name));
        assert_eq!(
            again, result,
            "{}: the result example drops or invents a field",
            m.name
        );
    }
}

#[test]
fn the_worked_example_is_a_valid_conversation() {
    let doc = parse(DOCUMENT);
    assert!(doc.exchange.len() >= 6, "the worked example is missing");
    let mut asked: Vec<(Id, &'static str)> = Vec::new();
    let mut subscribed = false;
    for (who, line) in &doc.exchange {
        let message = Message::parse(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert_eq!(
            &message.to_line(),
            line,
            "write the line the way the types write it"
        );
        match (who.as_str(), message) {
            ("client", Message::Request(r)) => asked.push((r.id, r.call.method())),
            ("daemon", Message::Response(r)) => {
                let (want, method) = asked.remove(0);
                assert_eq!(r.id, Some(want));
                match r.outcome {
                    Ok(v) => drop(reserialize_result(method, v).unwrap()),
                    Err(e) => {
                        let info = METHODS.iter().find(|m| m.name == method).unwrap();
                        assert!(info.errors.contains(&e.code), "{method} cannot return {e}");
                    }
                }
                subscribed |= method == "events.subscribe";
            }
            ("daemon", Message::Notice(_)) => {
                assert!(subscribed, "a notification before subscribing")
            }
            (who, other) => panic!("{who} cannot send {other:?}"),
        }
    }
    assert!(asked.is_empty(), "a request was never answered");
}

/// Prints the Methods, Notifications and Worked example sections from the samples, for pasting
/// into `PROTOCOL.md` after a change: `X100_REFERENCE=/path cargo test -p x100-proto print_reference
/// -- --ignored`.
#[test]
#[ignore = "writes a file"]
fn print_reference() {
    let Ok(out) = std::env::var("X100_REFERENCE") else {
        return;
    };
    let samples = samples();
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap();
    let mut o = String::new();
    writeln!(o, "## Methods\n").unwrap();
    for m in METHODS {
        let s = samples.iter().find(|s| s.name == m.name).unwrap();
        let errors = if m.errors.is_empty() {
            "none".to_owned()
        } else {
            m.errors
                .iter()
                .map(|e| e.name())
                .collect::<Vec<_>>()
                .join(", ")
        };
        writeln!(o, "### `{}`\n", m.name).unwrap();
        let summary = m
            .summary
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(o, "{summary}\n").unwrap();
        writeln!(
            o,
            "Params type `{}`, result type `{}`.\n",
            m.params.replace(' ', ""),
            m.result.replace(' ', "")
        )
        .unwrap();
        writeln!(o, "Errors: {errors}\n").unwrap();
        writeln!(o, "```json params\n{}\n```\n", pretty(&s.params)).unwrap();
        writeln!(o, "```json result\n{}\n```\n", pretty(&s.result)).unwrap();
    }
    writeln!(o, "## Notifications\n").unwrap();
    for (name, summary) in NOTICES {
        let notice = notice_sample(name);
        writeln!(o, "### `{name}`\n").unwrap();
        let summary = summary.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        writeln!(o, "{summary}\n").unwrap();
        writeln!(o, "```json notice\n{}\n```\n", pretty(&notice.params())).unwrap();
    }
    writeln!(o, "{}", worked_example()).unwrap();
    std::fs::write(out, o).unwrap();
}

fn worked_example() -> String {
    use x100_core::testing::fernleaf;
    let log = fernleaf().log;
    let prj = id(x100_core::id::ProjectId::new, 1);
    let agt = log.iter().find_map(|e| match &e.event {
        x100_core::event::ProjectEvent::Created { ceo, .. } => Some(*ceo),
        _ => None,
    });
    let n = |k: u64| Id::Number(k);
    let req = |k: u64, c: Call| Message::Request(Request { id: n(k), call: c });
    let ok = |k: u64, method: &str, v: Value| {
        reserialize_result(method, v.clone()).unwrap();
        Message::Response(Response {
            id: Some(n(k)),
            outcome: Ok(v),
        })
    };
    let sample = |name: &str| samples().into_iter().find(|s| s.name == name).unwrap();
    let steps: Vec<(&str, &str, Message)> = vec![
        (
            "The client says hello. The daemon answers with the version both will use.",
            "client",
            req(1, Call::parse("hello", sample("hello").params).unwrap()),
        ),
        ("", "daemon", ok(1, "hello", sample("hello").result)),
        (
            "The client subscribes with no cursor. The result carries the snapshot: the daemon's state and each project's, here a project at its first event.",
            "client",
            req(
                2,
                Call::parse("events.subscribe", sample("events.subscribe").params).unwrap(),
            ),
        ),
        (
            "",
            "daemon",
            ok(2, "events.subscribe", sample("events.subscribe").result),
        ),
        (
            "Every event after the snapshot follows as a notification, in order, once. The next two events of the project's log are 2 and 3.",
            "daemon",
            Message::Notice(Notice::Event(EventNotice::Project {
                subscription: SubscriptionId(1),
                project: prj,
                envelope: log[1].clone(),
            })),
        ),
        (
            "",
            "daemon",
            Message::Notice(Notice::Event(EventNotice::Project {
                subscription: SubscriptionId(1),
                project: prj,
                envelope: log[2].clone(),
            })),
        ),
        (
            "The builder opens an agent. The client follows its transcript from the start. The same connection carries both subscriptions.",
            "client",
            req(
                3,
                Call::from(SubscribeTranscript {
                    project: prj,
                    agent: agt.unwrap(),
                    after: None,
                }),
            ),
        ),
        (
            "",
            "daemon",
            ok(
                3,
                "transcript.subscribe",
                sample("transcript.subscribe").result,
            ),
        ),
        (
            "",
            "daemon",
            Message::Notice(notice_sample("transcript.entry")),
        ),
        (
            "The builder denies an action that was already decided. The daemon answers with a typed error.",
            "client",
            req(
                4,
                Call::from(DecideAction {
                    project: prj,
                    action: id(x100_core::id::ActionId::new, 10),
                    decision: x100_core::action::Decision::Denied,
                }),
            ),
        ),
        (
            "",
            "daemon",
            Message::Response(Response::err(
                Some(n(4)),
                RpcError::new(ErrorCode::WrongState, "action act_01KA... is not held"),
            )),
        ),
        (
            "The view closes, and the client ends the transcript subscription.",
            "client",
            req(
                5,
                Call::from(Unsubscribe {
                    subscription: SubscriptionId(2),
                }),
            ),
        ),
        ("", "daemon", ok(5, "unsubscribe", serde_json::json!({}))),
    ];
    let mut o = String::from("## Worked example\n\n");
    for (caption, who, message) in steps {
        if !caption.is_empty() {
            writeln!(o, "{caption}\n").unwrap();
        }
        writeln!(o, "```json {who}\n{}\n```\n", message.to_line()).unwrap();
    }
    o
}

fn notice_sample(name: &str) -> Notice {
    use x100_core::testing::fernleaf;
    let sub = SubscriptionId(1);
    match name {
        "events.event" => Notice::Event(EventNotice::Project {
            subscription: sub,
            project: id(x100_core::id::ProjectId::new, 1),
            envelope: fernleaf().log.remove(2),
        }),
        "transcript.entry" => Notice::TranscriptEntry(TranscriptNotice {
            subscription: SubscriptionId(2),
            position: Pos(42),
            entry: transcript::Entry {
                at: crate::samples::at(30),
                driver: id(x100_core::id::DriverId::new, 4),
                item: Item::ToolCall {
                    call: "toolu_01".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "cargo test"}),
                },
            },
        }),
        "terminal.chunk" => Notice::TerminalChunk(TerminalNotice {
            subscription: SubscriptionId(3),
            position: x100_core::terminal::TermPos(3),
            chunk: x100_core::terminal::Chunk {
                at: crate::samples::at(31),
                what: x100_core::terminal::ChunkKind::Output {
                    bytes: Bytes(b"cargo test\r\n".to_vec()),
                },
            },
        }),
        "subscription.closed" => Notice::Closed(SubscriptionClosed {
            subscription: sub,
            because: ClosedBecause::Lagged,
        }),
        other => panic!("no sample for {other}; add one"),
    }
}
