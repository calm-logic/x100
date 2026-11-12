//! The scripted fake harness, run as a real child process: replay, branching, posing and input.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::fake::*;
use common::*;
use serde_json::json;
use x100_core::driver::{ContextFill, Failure};
use x100_core::effort::Effort;
use x100_core::harness::HarnessKind;
use x100_core::id::DriverId;
use x100_core::ledger::CostBasis;
use x100_core::money::{Money, Usage};
use x100_drivers::fake::FakeHarness;
use x100_drivers::{
    Decision, DriverOutput, Harness, InputError, Permissions, Spent, StartError, TurnEnd,
};

const REPLAY: &str = r#"
[[turn]]
steps = [
  { text = "Reading the brief: {{input}}" },
  { call = { set_preview = { url = "http://localhost:3000" } }, save = "preview" },
  { text = "Preview: {{preview}}" },
  { call = { remember = { note = "n", body = "b" } }, save = "memory" },
  { text = "Memory: {{memory.note}} {{memory.n}}" },
  { write = "out/hello.txt", content = "hello {{preview}}" },
  { usage = { input = 1000, output = 200, cache_read = 5000 } },
  { context = 6200 },
]

[[turn]]
steps = [
  { text = "second" },
  { usage = { input = 10, output = 5, cost_micros = 42 } },
]
"#;

async fn replay_once() -> (Vec<DriverOutput>, String, usize) {
    let s = setup(REPLAY);
    let mut spec = spec_in(&s.work);
    spec.tools = Some(endpoint("http://127.0.0.1:1/mcp"));
    let mut handle = s.harness.start(spec).await.unwrap();
    assert_eq!(handle.info().version, "2.0.0");
    handle.turn("build it").await.unwrap();
    let mut seen = until_end(&mut handle).await;
    handle.turn("again").await.unwrap();
    seen.extend(until_end(&mut handle).await);
    handle.stop().await;
    assert_eq!(
        next(&mut handle).await,
        None,
        "a stopped driver's stream ends without a failure"
    );
    let written = std::fs::read_to_string(s.work.join("out/hello.txt")).unwrap();
    let calls = s.tools.calls.lock().unwrap();
    for (driver, endpoint, _) in calls.iter() {
        assert_eq!(*driver, DriverId::from_u128(1));
        assert_eq!(
            endpoint.as_ref().map(|e| e.token.as_str()),
            Some("drv-token-123")
        );
    }
    (seen, written, calls.len())
}

#[tokio::test]
async fn the_fake_replays_a_script_deterministically() {
    let (first, written, calls) = replay_once().await;
    let expected = vec![
        DriverOutput::Text("Reading the brief: build it".into()),
        DriverOutput::ToolCall {
            id: "call-1".into(),
            name: "set_preview".into(),
            input: json!({ "set_preview": { "url": "http://localhost:3000" } }),
        },
        DriverOutput::ToolResult {
            id: "call-1".into(),
            output: "previewing".into(),
            is_error: false,
        },
        DriverOutput::Text("Preview: previewing".into()),
        DriverOutput::ToolCall {
            id: "call-2".into(),
            name: "remember".into(),
            input: json!({ "remember": { "note": "n", "body": "b" } }),
        },
        DriverOutput::ToolResult {
            id: "call-2".into(),
            output: r#"{"n":7,"note":"kept"}"#.into(),
            is_error: false,
        },
        DriverOutput::Text("Memory: kept 7".into()),
        DriverOutput::ToolCall {
            id: "write-3".into(),
            name: "write".into(),
            input: json!({ "path": "out/hello.txt" }),
        },
        DriverOutput::ToolResult {
            id: "write-3".into(),
            output: "wrote 16 bytes to out/hello.txt".into(),
            is_error: false,
        },
        // 1000 at $1, 200 at $5 and 5000 at $0.10 per million: $0.0025.
        DriverOutput::Spent(Spent {
            usage: Usage {
                input: 1000,
                output: 200,
                cache_read: 5000,
                ..Usage::default()
            },
            cost: Money::from_micros(2500),
            basis: CostBasis::Listed,
        }),
        DriverOutput::Context(ContextFill {
            used: 6200,
            window: 200_000,
        }),
        DriverOutput::TurnEnded(TurnEnd::Completed),
        DriverOutput::Text("second".into()),
        DriverOutput::Spent(Spent {
            usage: Usage {
                input: 10,
                output: 5,
                ..Usage::default()
            },
            cost: Money::from_micros(42),
            basis: CostBasis::Reported,
        }),
        DriverOutput::TurnEnded(TurnEnd::Completed),
    ];
    assert_eq!(first, expected);
    assert_eq!(written, "hello previewing");
    assert_eq!(calls, 2);
    let (second, _, _) = replay_once().await;
    assert_eq!(
        second, first,
        "a second run of the same script is identical"
    );
}

const BRANCHING: &str = r#"
[[turn]]
when = { start_pack_contains = "ROLE: CEO", input_contains = "brief" }
steps = [{ text = "the CEO read the brief" }]

[[turn]]
when = { start_pack_contains = "ROLE: lead" }
repeat = true
steps = [{ text = "the lead got: {{input}}" }]

[[turn]]
when = { input_contains = "report" }
steps = [{ text = "a report arrived" }]
"#;

fn texts(outputs: &[DriverOutput]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_script_branches_on_markers_in_the_start_pack_and_the_input() {
    let s = setup(BRANCHING);
    let mut ceo_spec = spec_in(&s.work);
    ceo_spec.start_pack = "ROLE: CEO of the project".into();
    let mut ceo = s.harness.start(ceo_spec).await.unwrap();
    ceo.turn("the report from a lead").await.unwrap();
    assert_eq!(texts(&until_end(&mut ceo).await), ["a report arrived"]);
    ceo.turn("here is the brief").await.unwrap();
    assert_eq!(
        texts(&until_end(&mut ceo).await),
        ["the CEO read the brief"]
    );
    // Both entries a CEO can match are used up, and nothing repeats for it.
    ceo.turn("another brief").await.unwrap();
    match until_end(&mut ceo).await.last() {
        Some(DriverOutput::Failed(Failure::Error {
            detail,
            provider: None,
        })) => {
            assert!(detail.contains("no turn left"), "{detail}");
        }
        other => panic!("expected a failure saying the script ran out, got {other:?}"),
    }

    let mut lead_spec = spec(2, &s.work, "fake-small", Effort::Low);
    lead_spec.start_pack = "ROLE: lead, design".into();
    let mut lead = s.harness.start(lead_spec).await.unwrap();
    for input in ["first task", "second task"] {
        lead.turn(input).await.unwrap();
        assert_eq!(
            texts(&until_end(&mut lead).await),
            [format!("the lead got: {input}")]
        );
    }
    lead.stop().await;
}

const LOOKING_UP: &str = r#"
[[turn]]
when = { input_contains = "Task:" }
steps = [
  { text = "I am {{pack:Your id}}, on {{input:Task}} from {{input:Delegated by}}" },
  { call = { set_preview = { url = "http://localhost/{{input:Task}}" } } },
  { text = "next: {{input:Nothing here}}" },
  { text = "never said" },
]
"#;

#[tokio::test]
async fn a_script_reads_labelled_lines_of_its_input_and_start_pack() {
    let s = setup(LOOKING_UP);
    let mut lookup_spec = spec_in(&s.work);
    lookup_spec.start_pack = "ROLE: Tech\nYour id: agt_7\n\nCHARTER:\nBuild.\n".into();
    let mut handle = s.harness.start(lookup_spec).await.unwrap();
    handle
        .turn("A task for you.\n  Task: tsk_3 (Build the site)\nDelegated by: agt_1 (CEO)\nTask: tsk_9")
        .await
        .unwrap();
    let seen = until_end(&mut handle).await;
    // The first matching line gives the first word of its value, indented or not.
    assert_eq!(texts(&seen), ["I am agt_7, on tsk_3 from agt_1"]);
    let calls = s.tools.calls.lock().unwrap().clone();
    assert!(
        matches!(&calls[0].2, x100_core::tool::ToolCall::SetPreview { url } if url == "http://localhost/tsk_3")
    );
    // A label the input lacks fails the driver, and says which lookup found nothing.
    match seen.last() {
        Some(DriverOutput::Failed(Failure::Error {
            detail,
            provider: None,
        })) => assert_eq!(
            detail,
            "{{input:Nothing here}}: the turn's input has no line \"Nothing here: ...\""
        ),
        other => panic!("expected a failure naming the lookup, got {other:?}"),
    }
    handle.stop().await;
}

#[tokio::test]
async fn one_binary_poses_as_two_providers_with_their_own_models_and_prices() {
    let other = r#"
[harness]
name = "fake-openai"
[[model]]
model = "fake-mini"
context_window = 100000
efforts = ["low", "high"]
price = { input = 2.0, output = 8.0 }
[[turn]]
steps = [{ usage = { input = 1000, output = 1000 } }]
"#;
    let a = setup("[[turn]]\nsteps = [{ usage = { input = 1000, output = 1000 } }]");
    let b_dir = tempfile::tempdir().unwrap();
    std::fs::write(b_dir.path().join("b.toml"), other).unwrap();
    let b = FakeHarness::new(
        FAKE,
        b_dir.path().join("b.toml"),
        Arc::new(a.clock.clone()),
        None,
    )
    .unwrap();
    assert_eq!(
        a.harness.kind(),
        HarnessKind::Custom {
            name: "fake-anthropic".into()
        }
    );
    assert_eq!(
        b.kind(),
        HarnessKind::Custom {
            name: "fake-openai".into()
        }
    );
    assert_eq!(
        b.models()[0]
            .levels
            .iter()
            .map(|(e, _)| *e)
            .collect::<Vec<_>>(),
        [Effort::Low, Effort::High]
    );

    let mut cost = Vec::new();
    for (harness, model, effort) in [
        (&a.harness, "fake-small", Effort::Medium),
        (&b, "fake-mini", Effort::High),
    ] {
        let mut handle = harness
            .start(spec(9, &a.work, model, effort))
            .await
            .unwrap();
        handle.turn("go").await.unwrap();
        for out in until_end(&mut handle).await {
            if let DriverOutput::Spent(spent) = out {
                cost.push(spent.cost);
            }
        }
    }
    // 1000 in and 1000 out: $1 + $5 per million, against $2 + $8.
    assert_eq!(cost, [Money::from_micros(6000), Money::from_micros(10_000)]);
    assert!(matches!(
        b.start(spec(10, &a.work, "fake-mini", Effort::Medium))
            .await,
        Err(StartError::NotOffered { .. })
    ));
}

const ASKING: &str = r#"[[turn]]
repeat = true
steps = [{ ask = "Bash", input = { command = "rm -rf build" } }, { text = "after" }]"#;

#[tokio::test]
async fn a_permission_request_waits_for_the_daemons_answer() {
    let s = setup(ASKING);
    let mut ask_spec = spec_in(&s.work);
    ask_spec.permissions = Permissions::AskDaemon;
    let mut handle = s.harness.start(ask_spec).await.unwrap();
    let control = handle.control();
    handle.turn("go").await.unwrap();
    let (_, asked) = until(&mut handle, |o| {
        matches!(o, DriverOutput::PermissionRequest(_))
    })
    .await;
    let DriverOutput::PermissionRequest(request) = asked else {
        unreachable!()
    };
    assert_eq!(request.tool, "Bash");
    assert_eq!(request.input, json!({ "command": "rm -rf build" }));
    // Waiting on the daemon is not the driver's silence.
    s.clock.advance(Duration::from_secs(3600));
    assert!(!control.check_stall().await);
    control
        .answer(
            request.id.clone(),
            Decision::Deny {
                reason: "held for the builder".into(),
            },
        )
        .await
        .unwrap();
    let rest = until_end(&mut handle).await;
    assert!(rest.contains(&DriverOutput::ToolResult {
        id: "tool-1".into(),
        output: "denied: held for the builder".into(),
        is_error: true,
    }));
    assert_eq!(
        control.answer(request.id, Decision::Allow).await,
        Err(InputError::NoSuchRequest(x100_drivers::PermissionId(
            "ask-2".into()
        )))
    );

    // With nothing to hold, the driver never asks.
    let mut handle = s
        .harness
        .start(spec(2, &s.work, "fake-small", Effort::Low))
        .await
        .unwrap();
    handle.turn("go").await.unwrap();
    let seen = until_end(&mut handle).await;
    assert!(
        seen.iter()
            .all(|o| !matches!(o, DriverOutput::PermissionRequest(_)))
    );
    assert!(
        seen.iter()
            .any(|o| matches!(o, DriverOutput::ToolResult { output, .. } if output == "allowed"))
    );
}

#[tokio::test]
async fn what_a_harness_lacks_is_refused_with_a_clear_error() {
    let s = setup(
        r#"[[turn]]
repeat = true
steps = [{ sleep_ms = 60000 }, { text = "never" }]"#,
    );
    let limited = format!(
        "{}\n[[turn]]\nsteps = [{{ text = \"once\" }}]",
        POSING.replace(
            "version = \"2.0.0\"",
            "version = \"2.0.0\"\nsupports = { further_turns = false, compaction = false, permission_hook = false }"
        )
    );
    let path = s.work.join("limited.toml");
    std::fs::write(&path, limited).unwrap();
    let limited = FakeHarness::new(FAKE, &path, Arc::new(s.clock.clone()), None).unwrap();

    let mut ask = spec_in(&s.work);
    ask.permissions = Permissions::AskDaemon;
    assert!(matches!(
        limited.start(ask).await,
        Err(StartError::Unsupported(_))
    ));
    let mut handle = limited.start(spec_in(&s.work)).await.unwrap();
    assert_eq!(
        handle.control().compact().await,
        Err(InputError::Unsupported("compact"))
    );
    handle.turn("one").await.unwrap();
    until_end(&mut handle).await;
    assert_eq!(
        handle.turn("two").await,
        Err(InputError::Unsupported("take a further turn"))
    );

    // A turn cannot start while one runs; an interrupt ends the running one.
    let mut handle = s
        .harness
        .start(spec(2, &s.work, "fake-small", Effort::Low))
        .await
        .unwrap();
    let control = handle.control();
    assert_eq!(control.interrupt().await, Err(InputError::NotInTurn));
    handle.turn("long").await.unwrap();
    assert_eq!(handle.turn("again").await, Err(InputError::Busy));
    assert_eq!(control.compact().await, Err(InputError::Busy));
    control.interrupt().await.unwrap();
    assert_eq!(
        until_end(&mut handle).await,
        [DriverOutput::TurnEnded(TurnEnd::Interrupted)]
    );
}

#[tokio::test]
async fn compaction_reports_the_context_left() {
    let s = setup("[compaction]\ncontext_after = 3000");
    let mut handle = s.harness.start(spec_in(&s.work)).await.unwrap();
    handle.control().compact().await.unwrap();
    assert_eq!(
        until_end(&mut handle).await,
        [
            DriverOutput::Context(ContextFill {
                used: 3000,
                window: 200_000
            }),
            DriverOutput::Compacted
        ]
    );
}

#[tokio::test]
async fn the_spawn_hook_is_called_with_a_live_pid_before_start_returns() {
    let s = setup(REPLAY);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut spec = spec_in(&s.work);
    spec.on_spawn = x100_drivers::SpawnHook::new({
        let seen = Arc::clone(&seen);
        move |pid| seen.lock().unwrap().push(pid)
    });
    let handle = s.harness.start(spec).await.unwrap();

    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [handle.info().pid],
        "called once, with the child's pid"
    );
    assert!(seen[0] > 0);
    // Signal 0 only asks whether the process exists; the child is still running.
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(seen[0] as i32), None).expect("alive");
    handle.stop().await;
}
