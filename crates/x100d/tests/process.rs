//! The daemon as a child process, and the client library as the only way in.

mod common;

use common::*;
use x100_core::driver::DriverState;
use x100_core::event::ProjectEvent;
use x100_core::message::Route;
use x100_proto::{LogId, LogPage, Page, ReadLog, SendChat};

fn say(project: x100_core::id::ProjectId, body: &str) -> SendChat {
    SendChat {
        project,
        body: body.into(),
        attachments: vec![],
        in_reply_to: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_to_the_ceo_is_answered_as_events() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "fernleaf").await;
    let sent = client.request(say(project, "ping")).await.unwrap();

    let reply = client
        .wait_for(WAIT, |s| {
            s.projects[&project]
                .messages()
                .find(|m| m.message.route == Route::CeoToBuilder)
                .cloned()
        })
        .await
        .expect("the reply arrives");
    // The project is not set up, so the message opens no task and the CEO only answers it.
    let tasks = client.with_state(|s| s.projects[&project].tasks().count());
    assert_eq!(tasks, 0);
    assert_eq!(
        reply.message.body,
        format!(
            "heard: A message from the builder.\nMessage: {}\nFrom: builder\nping",
            sent.message
        )
    );
    assert_eq!(reply.message.in_reply_to, Some(sent.message));

    // The turn is over and recorded: the driver is warm, the item handled, the cost charged.
    let done = client
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            let d = ceo_driver(p)?;
            (d.state == DriverState::Warm && p.agent(p.project()?.ceo)?.inbox.is_empty())
                .then(|| d.clone())
        })
        .await
        .expect("the driver is warm");
    assert_eq!(done.turns, 1);
    assert_eq!(done.spent.cost.micros(), 1234);
    assert!(done.context.is_some());

    // And the log says it in order.
    let page = client
        .request(ReadLog {
            log: LogId::Project(project),
            page: Page::OldestFirst {
                after: None,
                limit: 100,
            },
        })
        .await
        .unwrap();
    let LogPage::Project { events } = page else {
        panic!("a project's log")
    };
    let names: Vec<&str> = events
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::DriverStarted(_) => Some("started"),
            ProjectEvent::Driver { event, .. } => Some(event.name()),
            ProjectEvent::Charged(_) => Some("charged"),
            ProjectEvent::Checkpointed { .. } => Some("checkpointed"),
            ProjectEvent::MessageSent(m) if m.route == Route::CeoToBuilder => Some("reply"),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        [
            "started",
            "TurnStarted",
            "reply",
            "charged",
            "ContextReported",
            "checkpointed",
            "TurnEnded"
        ]
    );
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn killing_the_daemon_mid_turn_fails_that_driver_and_ends_the_warm_ones() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let busy = create_project(&client, &env, "busy").await;
    let calm = create_project(&client, &env, "calm").await;
    client.request(say(busy, "SLEEP now")).await.unwrap();
    client.request(say(calm, "ping")).await.unwrap();

    // One driver is in the middle of a turn that will not end, the other is warm.
    let (busy_driver, calm_driver) = client
        .wait_for(WAIT, |s| {
            let b = ceo_driver(&s.projects[&busy])?;
            let c = ceo_driver(&s.projects[&calm])?;
            (b.state == DriverState::Running && c.state == DriverState::Warm)
                .then_some((b.driver.id, c.driver.id))
        })
        .await
        .expect("one driver mid-turn and one warm");
    let recorded = env.recorded_pids();
    let pids: Vec<u32> = recorded.iter().map(|(_, p)| *p).collect();
    assert_eq!(pids.len(), 2, "both driver processes are written down");
    let busy_pid = recorded
        .iter()
        .find(|(d, _)| *d == busy_driver.to_string())
        .unwrap()
        .1;
    // What the busy driver left running in its group, which is the part that can outlive it.
    let mut stragglers = Vec::new();
    for _ in 0..200 {
        stragglers = group_members(busy_pid)
            .into_iter()
            .filter(|p| *p != busy_pid)
            .collect();
        if !stragglers.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        !stragglers.is_empty(),
        "the script started a background process"
    );
    let anchors: Vec<u32> = env.recorded_sandboxes().iter().map(|(_, p)| *p).collect();
    assert_eq!(anchors.len(), 2, "each CEO's sandbox is written down");
    drop(client);

    daemon.kill9();
    // A killed daemon cannot stop what its drivers started, but each driver runs in its agent's
    // sandbox, which ends when the daemon's end of its tunnel closes and takes everything in it
    // along: the background process too, before any daemon starts again.
    gone(
        &[busy_pid, pids[0], pids[1]]
            .into_iter()
            .chain(stragglers.iter().copied())
            .chain(anchors)
            .collect::<Vec<_>>(),
    )
    .await;
    let mut daemon = env.spawn();
    let client = env.client().await;

    let s = client.state();
    let before = |project, id| {
        s.projects[&project]
            .driver(id)
            .expect("the old driver is in the log")
            .state
            .clone()
    };
    assert_eq!(
        before(busy, busy_driver),
        DriverState::Failed(x100_core::driver::Failure::DaemonRestarted)
    );
    assert_eq!(
        before(calm, calm_driver),
        DriverState::Ended(x100_core::driver::EndReason::Stopped(
            x100_core::driver::StopReason::DaemonRestarted
        ))
    );
    for old in [(busy, busy_driver), (calm, calm_driver)] {
        assert!(!before(old.0, old.1).is_live());
    }

    // The mid-turn agent still has its message, so it gets a new driver by the ordinary rule.
    let again = client
        .wait_for(WAIT, |s| {
            let d = ceo_driver(&s.projects[&busy])?;
            (d.driver.id != busy_driver && d.state == DriverState::Running).then_some(d.driver.id)
        })
        .await
        .expect("a new driver takes the unhandled message");
    assert_ne!(again, busy_driver);

    // A new message to the other CEO gets a reply, on a new driver.
    client.request(say(calm, "ping again")).await.unwrap();
    let reply = client
        .wait_for(WAIT, |s| {
            s.projects[&calm]
                .messages()
                .find(|m| {
                    m.message.body.contains("ping again") && m.message.route == Route::CeoToBuilder
                })
                .cloned()
        })
        .await
        .expect("a reply");
    assert!(reply.message.body.starts_with("heard:"));
    let driver = client
        .wait_for(WAIT, |s| {
            let d = ceo_driver(&s.projects[&calm])?;
            (d.state == DriverState::Warm).then_some(d.driver.id)
        })
        .await
        .unwrap();
    assert_ne!(driver, calm_driver);

    // A clean stop settles the sleeping driver too, and leaves nothing running.
    let pids: Vec<u32> = env.recorded_pids().into_iter().map(|(_, p)| p).collect();
    assert_eq!(pids.len(), 2);
    let everything: Vec<u32> = pids.iter().flat_map(|p| group_members(*p)).collect();
    drop(client);
    daemon.terminate();
    gone(&everything).await;
    assert!(env.recorded_pids().is_empty());
    assert!(
        !env.socket().exists(),
        "the socket is removed on a clean stop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_see_the_same_events() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let (one, two) = (env.client().await, env.client().await);
    let project = create_project(&one, &env, "shared").await;
    one.request(say(project, "ping")).await.unwrap();
    for client in [&one, &two] {
        client
            .wait_for(WAIT, |s| {
                let p = s.projects.get(&project)?;
                let d = ceo_driver(p)?;
                (d.state == DriverState::Warm && p.messages().count() == 2).then_some(())
            })
            .await
            .expect("both see the whole exchange");
    }
    // A turn's end is several events written together, and either client may be part way
    // through them, or behind on the daemon's own log. Read where every log ends once, and
    // compare only when both clients have applied exactly that much of each.
    let daemon_last = one.request(x100_proto::GetDaemon {}).await.unwrap().seq();
    let project_last = one
        .request(x100_proto::GetProject { project })
        .await
        .unwrap()
        .seq();
    for client in [&one, &two] {
        client
            .wait_for(WAIT, |s| {
                (s.daemon.seq() == daemon_last && s.projects.get(&project)?.seq() == project_last)
                    .then_some(())
            })
            .await
            .expect("both reach the end of every log");
    }
    assert_eq!(one.state(), two.state());
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_reconnects_resumes_without_a_gap_or_a_repeat() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "busy").await;
    let first = client.with_state(|s| s.projects[&project].seq().unwrap());
    let mut changes = client.changes();

    // Events keep arriving while the connection is dropped again and again.
    let sender = {
        let client = client.clone();
        tokio::spawn(async move {
            for n in 0..30 {
                // A request in flight when the connection drops is refused, not retried by the
                // client: it may or may not have happened, and a duplicate message is harmless.
                while let Err(e) = client.request(say(project, &format!("message {n}"))).await {
                    assert!(matches!(e, x100_client::ClientError::Disconnected), "{e}");
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        })
    };
    while !sender.is_finished() {
        client.reconnect();
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    sender.await.unwrap();
    // The CEO answers, and has nothing left.
    client
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            let replies = p
                .messages()
                .filter(|m| m.message.route == Route::CeoToBuilder)
                .count();
            (replies >= 1 && p.agent(p.project()?.ceo)?.inbox.is_empty()).then_some(())
        })
        .await
        .expect("the CEO caught up");

    // Each event of the project's log was applied once: one change per event, and never the
    // replacement of the state, which is what a missed or repeated event would have caused.
    let fresh = env.client().await;
    let last = client
        .wait_for(WAIT, |s| {
            let held = &s.projects[&project];
            (fresh.state().projects.get(&project) == Some(held)).then(|| held.seq().unwrap())
        })
        .await
        .expect("a fresh snapshot equals the state folded across reconnects");
    let mut logged = 0;
    let mut reset = false;
    while let Ok(change) = changes.try_recv() {
        match change {
            x100_client::Change::Reset => reset = true,
            x100_client::Change::Project {
                project: p,
                changes,
            } if p == project => {
                logged += u64::from(changes.parts.contains(&x100_core::fold::ProjectPart::Log));
            }
            _ => {}
        }
    }
    assert!(!reset, "the state was replaced by a new snapshot");
    assert_eq!(
        logged,
        last.0 - first.0,
        "one change for each event, none missed or repeated"
    );
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_every_client_does_not_stop_a_running_turn() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "slow").await;
    client.request(say(project, "SLOW")).await.unwrap();
    let driver = client
        .wait_for(WAIT, |s| {
            let d = ceo_driver(&s.projects[&project])?;
            (d.state == DriverState::Running).then_some(d.driver.id)
        })
        .await
        .unwrap();
    drop(client);

    let again = env.client().await;
    let finished = again
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            let reply = p
                .messages()
                .find(|m| m.message.route == Route::CeoToBuilder)?;
            Some((reply.message.body.clone(), ceo_driver(p)?.clone()))
        })
        .await
        .expect("the turn went on without a client");
    // What it wrote as it worked stays in its transcript: only its message is a reply.
    assert_eq!(finished.0, "done thinking");
    assert_eq!(finished.1.driver.id, driver, "the same driver finished it");
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_transcript_subscription_delivers_a_turns_entries_in_order() {
    use x100_core::transcript::{Item, TurnEnd};
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "talk").await;
    let ceo = client.with_state(|s| s.projects[&project].project().unwrap().ceo);
    let mut stream = client.follow_transcript(project, ceo, None).await.unwrap();
    client.request(say(project, "ping")).await.unwrap();

    let mut got: Vec<(x100_core::transcript::Pos, x100_core::transcript::Entry)> = Vec::new();
    while !matches!(got.last(), Some((_, e)) if matches!(e.item, Item::TurnEnded { .. })) {
        let next = tokio::time::timeout(WAIT, stream.next())
            .await
            .expect("an entry")
            .expect("open");
        got.push(next);
    }
    let positions: Vec<u64> = got.iter().map(|(p, _)| p.0).collect();
    assert_eq!(positions, (1..=10).collect::<Vec<u64>>());
    let kinds: Vec<&str> = got
        .iter()
        .map(|(_, e)| match &e.item {
            Item::StartPack { text } => {
                assert!(text.starts_with("ROLE: CEO\n"), "{text}");
                "start_pack"
            }
            Item::DriverChanged { from: None } => "driver_changed",
            Item::TurnStarted => "turn_started",
            Item::Text { .. } => "text",
            Item::ToolCall { .. } => "tool_call",
            Item::ToolResult { .. } => "tool_result",
            Item::Usage { .. } => "usage",
            Item::TurnEnded {
                how: TurnEnd::Completed,
            } => "turn_ended",
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    // The script answers with `message` after its text, then writes a file before its usage: each
    // shows as a call and its result.
    assert_eq!(
        kinds,
        [
            "start_pack",
            "driver_changed",
            "turn_started",
            "text",
            "tool_call",
            "tool_result",
            "tool_call",
            "tool_result",
            "usage",
            "turn_ended"
        ]
    );
    // Reading the page back gives the same entries.
    let page = client
        .request(x100_proto::ReadTranscript {
            project,
            agent: ceo,
            after: None,
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 10);
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_soon_after_another_goes_to_the_same_warm_driver() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "reuse").await;
    client.request(say(project, "SLOW")).await.unwrap();
    client
        .wait_for(WAIT, |s| {
            (ceo_driver(&s.projects[&project])?.state == DriverState::Running).then_some(())
        })
        .await
        .unwrap();
    // This one arrives mid-turn: it waits, and is given to the same driver between turns.
    client.request(say(project, "ping")).await.unwrap();
    let ceo = client
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            let replies = p
                .messages()
                .filter(|m| m.message.route == Route::CeoToBuilder)
                .count();
            let d = ceo_driver(p)?;
            (replies == 2 && d.state == DriverState::Warm).then(|| d.clone())
        })
        .await
        .expect("both are answered");
    assert_eq!(ceo.turns, 2);
    assert_eq!(
        drivers(&client.state(), project).len(),
        1,
        "one driver for both"
    );
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_that_fails_is_recorded_with_its_reason_and_its_message_is_kept() {
    use x100_core::driver::Failure;
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "crash").await;
    client.request(say(project, "CRASH")).await.unwrap();
    // A driver that fails is replaced, and after three failures in a row the agent stops trying.
    // The project is not set up, so there is no task to report failed.
    let failed = client
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            let ceo = p.agent(p.project()?.ceo)?;
            let all_failed = ceo.drivers.len() == 3
                && ceo.drivers.iter().all(|d| {
                    p.driver(*d)
                        .is_some_and(|d| matches!(d.state, DriverState::Failed(_)))
                });
            all_failed.then(|| ceo_driver(p).cloned()).flatten()
        })
        .await
        .unwrap();
    assert_eq!(
        failed.state,
        DriverState::Failed(Failure::ProcessExited { code: Some(3) })
    );
    let ceo = client.with_state(|s| s.projects[&project].project().unwrap().ceo);
    let inbox = client
        .request(x100_proto::ReadInbox {
            project,
            agent: ceo,
        })
        .await
        .unwrap();
    assert_eq!(inbox.items.len(), 1);
    assert!(
        !inbox.items[0].being_handled,
        "the message waits for the next driver"
    );
    // Failed drivers are not started again without end: nothing new reached the agent.
    assert_eq!(drivers(&client.state(), project).len(), 3);
    let page = client
        .request(x100_proto::ReadTranscript {
            project,
            agent: ceo,
            after: None,
            limit: 100,
        })
        .await
        .unwrap();
    let last = page.entries.last().unwrap();
    assert!(matches!(
        &last.entry.item,
        x100_core::transcript::Item::TurnEnded {
            how: x100_core::transcript::TurnEnd::Failed { .. }
        }
    ));
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_from_a_driver_is_carried_out_and_recorded() {
    use x100_core::transcript::Item;
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "tools").await;
    client.request(say(project, "TOOL")).await.unwrap();
    let ceo = client.with_state(|s| s.projects[&project].project().unwrap().ceo);
    let mut stream = client.follow_transcript(project, ceo, None).await.unwrap();
    loop {
        let (_, entry) = tokio::time::timeout(WAIT, stream.next())
            .await
            .unwrap()
            .unwrap();
        if let Item::ToolResult {
            output, is_error, ..
        } = entry.item
        {
            assert!(!is_error, "{output}");
            assert!(output.contains("http://localhost:1"), "{output}");
            break;
        }
    }
    client
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            let carried_out = p
                .actions()
                .any(|a| a.state == x100_core::action::ActionState::CarriedOut);
            (p.project()?.preview.as_deref() == Some("http://localhost:1") && carried_out)
                .then_some(())
        })
        .await
        .expect("the preview is set, and the call is an action carried out");
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_driver_ends_it_and_its_message_goes_to_the_next() {
    use x100_core::driver::{EndReason, StopReason};
    use x100_proto::{ErrorCode, StopDriver};
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, "stop").await;
    client.request(say(project, "SLEEP")).await.unwrap();
    let first = client
        .wait_for(WAIT, |s| {
            let d = ceo_driver(&s.projects[&project])?;
            (d.state == DriverState::Running).then_some(d.driver.id)
        })
        .await
        .unwrap();
    let pid = env.recorded_pids()[0].1;
    client
        .request(StopDriver {
            project,
            driver: first,
        })
        .await
        .unwrap();
    gone(&[pid]).await;
    let s = client
        .wait_for(WAIT, |s| {
            let d = s.projects[&project].driver(first)?;
            (!d.state.is_live()).then(|| d.state.clone())
        })
        .await
        .unwrap();
    assert_eq!(
        s,
        DriverState::Ended(EndReason::Stopped(StopReason::ReplacedByBuilder))
    );
    let second = client
        .wait_for(WAIT, |s| {
            let d = ceo_driver(&s.projects[&project])?;
            (d.driver.id != first && d.state == DriverState::Running).then_some(d.driver.id)
        })
        .await
        .expect("the message went to a new driver");
    let again = client
        .request(StopDriver {
            project,
            driver: first,
        })
        .await
        .unwrap_err();
    assert!(matches!(again, x100_client::ClientError::Rpc(e) if e.code == ErrorCode::WrongState));
    client
        .request(StopDriver {
            project,
            driver: second,
        })
        .await
        .unwrap();
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_finds_no_daemon_starts_one() {
    let env = Env::new();
    let mut options = x100_client::Options::new(env.socket(), "test");
    options.launch = Some(x100_client::Launch {
        program: env!("CARGO_BIN_EXE_x100d").into(),
        args: env.daemon_args(std::path::Path::new(env!(
            "CARGO_BIN_EXE_x100d-fake-harness"
        ))),
    });
    options.startup_timeout = WAIT;
    let client = x100_client::Client::connect(options)
        .await
        .expect("a daemon was started");
    let project = create_project(&client, &env, "auto").await;
    drop(client);
    // The daemon outlives the client that started it.
    let again = env.client().await;
    assert!(again.state().projects.contains_key(&project));
    let pid: i32 = std::fs::read_to_string(env.data.join("x100d.pid"))
        .unwrap()
        .parse()
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    gone(&[pid as u32]).await;
}

/// The fake harness cannot pause in its handshake by script, so the program the daemon starts is a
/// shell script that sleeps first and then becomes the fake harness. The sleeping script is the
/// driver's process: the daemon has spawned it and the handshake cannot finish.
#[tokio::test(flavor = "multi_thread")]
async fn a_driver_killed_with_the_daemon_during_its_handshake_is_reaped_on_the_next_start() {
    let env = Env::new();
    let slow = env.dir.path().join("slow-harness.sh");
    std::fs::write(
        &slow,
        format!(
            "#!/bin/sh\nsleep 600\nexec {} \"$@\"\n",
            env!("CARGO_BIN_EXE_x100d-fake-harness")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&slow, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let mut daemon = env.spawn_with(&slow);
    let client = env.client().await;
    // Without a sandbox, as the daemon's setting allows: in one, the driver would end with the
    // daemon, and this is about the list the next daemon reaps from.
    client
        .request(x100_proto::SetDaemonSetting {
            setting: x100_core::settings::DaemonSetting::RunWithoutSandbox(true),
        })
        .await
        .unwrap();
    let project = create_project(&client, &env, "slow").await;
    client.request(say(project, "ping")).await.unwrap();

    // The process is written down while it is still asleep, long before any handshake.
    let mut recorded = Vec::new();
    for _ in 0..400 {
        recorded = env.recorded_pids();
        if !recorded.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(recorded.len(), 1, "the driver is written down at spawn");
    let pid = recorded[0].1;
    let members = group_members(pid);
    assert!(members.contains(&pid) && members.len() >= 2, "{members:?}");
    assert!(
        ceo_driver(&client.state().projects[&project])
            .is_some_and(|d| d.state != DriverState::Warm && d.state != DriverState::Running),
        "the driver has not finished starting"
    );
    drop(client);

    daemon.kill9();
    assert!(running(pid), "a killed daemon cannot stop what it started");
    let _daemon = env.spawn();
    let _client = env.client().await;
    gone(&members).await;
}
