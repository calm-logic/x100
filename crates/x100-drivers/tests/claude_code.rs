//! The real Claude Code binary, driven end to end against the fake provider on localhost. No
//! request leaves the machine: the base URL is the fake provider, the key is a dummy, the
//! environment is built from nothing, and the proxy is the fake provider, which refuses and
//! records any attempt to reach another host.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use serde_json::json;
use x100_core::driver::Failure;
use x100_core::effort::Effort;
use x100_core::harness::ProviderFailure;
use x100_core::ledger::CostBasis;
use x100_core::money::Usage;
use x100_core::time::{FakeClock, Timestamp};
use x100_drivers::claude::{ClaudeCode, ClaudeCodeConfig, models};
use x100_drivers::testing::{
    FakeProvider, REPLY_CACHE_READ, REPLY_CACHE_WRITE, REPLY_INPUT, REPLY_OUTPUT, Reply,
};
use x100_drivers::{
    Decision, DriverHandle, DriverOutput, DriverSpec, Harness, Permissions, TurnEnd, group_is_empty,
};

const KEY: &str = "sk-ant-x100-fake-key";
const MODEL: &str = "claude-haiku-5-5";

struct Setup {
    _dir: tempfile::TempDir,
    work: PathBuf,
    home: PathBuf,
    provider: FakeProvider,
    harness: ClaudeCode,
}

async fn setup() -> Option<Setup> {
    let program = installed("claude")?;
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let home = dir.path().join("home");
    for d in [&work, &home] {
        std::fs::create_dir(d).unwrap();
    }
    let provider = FakeProvider::start().await.unwrap();
    let mut config = ClaudeCodeConfig::new();
    config.program = program;
    config.base_url = Some(provider.url());
    config.inherit_env = vec!["PATH".into()];
    let clock = FakeClock::new(Timestamp::from_millis(1));
    Some(Setup {
        _dir: dir,
        work,
        home,
        provider,
        harness: ClaudeCode::new(config, Arc::new(clock)),
    })
}

impl Setup {
    fn spec(&self, n: u128) -> DriverSpec {
        let mut spec = spec(n, &self.work, MODEL, Effort::Low);
        spec.env = vec![
            ("HOME".into(), self.home.display().to_string()),
            ("ANTHROPIC_API_KEY".into(), KEY.into()),
            ("HTTPS_PROXY".into(), self.provider.url()),
            ("HTTP_PROXY".into(), self.provider.url()),
            ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
        ];
        spec
    }

    async fn start(&self, spec: DriverSpec) -> DriverHandle {
        let handle = self.harness.start(spec).await.unwrap();
        // Claude Code updates itself, so the installed version is whatever this machine has.
        // A version other than the tested one is allowed its one warning, and the tests that
        // follow say whether the newer program still behaves.
        let other: Vec<_> = handle
            .info()
            .warnings
            .iter()
            .filter(|w| !w.contains("is not the version x100 was tested with"))
            .collect();
        assert!(other.is_empty(), "{other:?}");
        handle
    }

    fn assert_nothing_left_the_machine(&self) {
        assert_eq!(self.provider.proxy_attempts(), Vec::<String>::new());
    }
}

fn spent(outputs: &[DriverOutput]) -> Vec<x100_drivers::Spent> {
    outputs
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Spent(s) => Some(*s),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn claude_edits_a_file_reports_usage_and_keeps_context_on_a_second_turn() {
    let Some(s) = setup().await else { return };
    let file = s.work.join("hello.txt");
    s.provider.reply(Reply::Tool {
        name: "Edit".into(),
        input: json!({ "file_path": file, "old_string": "", "new_string": "hello from x100\n" }),
    });
    s.provider.reply(Reply::Text("Edited.".into()));
    let mut spec = s.spec(1);
    spec.start_pack = "You are the x100 test agent. START-PACK-MARKER".into();
    spec.tools = Some(endpoint(&s.provider.mcp_url()));
    let mut handle = s.start(spec).await;

    handle
        .turn("Create hello.txt. FIRST-TURN-MARKER")
        .await
        .unwrap();
    let first = until_end(&mut handle).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello from x100\n");
    assert!(
        matches!(&first[0], DriverOutput::ToolCall { name, .. } if name == "Edit"),
        "{first:#?}"
    );
    assert!(
        matches!(
            &first[1],
            DriverOutput::ToolResult {
                is_error: false,
                ..
            }
        ),
        "{first:#?}"
    );
    assert_eq!(first[2], DriverOutput::Text("Edited.".into()));
    assert!(
        matches!(first[first.len() - 2], DriverOutput::Context(c) if c.window == 1_000_000 && c.used > 0)
    );
    assert_eq!(
        first.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed))
    );
    // Two model requests, each reporting the fake provider's tokens.
    let usage = Usage {
        input: 2 * REPLY_INPUT,
        output: 2 * REPLY_OUTPUT,
        cache_read: 2 * REPLY_CACHE_READ,
        cache_write_short: 2 * REPLY_CACHE_WRITE,
        cache_write_long: 0,
    };
    let turn_spent = spent(&first);
    assert_eq!(turn_spent.len(), 1);
    assert_eq!(turn_spent[0].usage, usage);
    assert_eq!(turn_spent[0].basis, CostBasis::Reported);
    // Claude Code's own figure agrees with x100's table for the model.
    let listed = models()
        .into_iter()
        .find(|m| m.model.0 == MODEL)
        .unwrap()
        .levels[0]
        .1;
    assert_eq!(turn_spent[0].cost, listed.cost(&usage));

    let requests = s.provider.requests();
    assert_eq!(requests[0].header("x-api-key"), Some(KEY));
    assert!(
        requests[0].body["system"]
            .to_string()
            .contains("START-PACK-MARKER")
    );
    assert!(
        s.provider
            .mcp_calls()
            .iter()
            .any(|c| c.method == "initialize"
                && c.authorization.as_deref() == Some("Bearer drv-token-123"))
    );

    // The second turn goes to the same process, which sends the first turn back to the model,
    // and calls one of the daemon's tools without asking anyone.
    s.provider.reply(Reply::Tool {
        name: "mcp__x100__ping".into(),
        input: json!({ "msg": "hi" }),
    });
    s.provider.reply(Reply::Text("Second turn.".into()));
    handle
        .turn("What did you do? SECOND-TURN-MARKER")
        .await
        .unwrap();
    let second = until_end(&mut handle).await;
    assert!(
        second.contains(&DriverOutput::Text("Second turn.".into())),
        "{second:#?}"
    );
    assert!(second.iter().any(|o| matches!(o, DriverOutput::ToolResult { output, is_error: false, .. } if output == "pong")), "{second:#?}");
    let last = s.provider.requests().last().unwrap().body.to_string();
    for earlier in ["FIRST-TURN-MARKER", "Edited.", "SECOND-TURN-MARKER"] {
        assert!(
            last.contains(earlier),
            "the second turn's request lacks {earlier}"
        );
    }
    assert!(
        s.provider
            .mcp_calls()
            .iter()
            .any(|c| c.method == "tools/call"
                && c.authorization.as_deref() == Some("Bearer drv-token-123"))
    );
    assert!(
        first
            .iter()
            .chain(&second)
            .all(|o| !matches!(o, DriverOutput::PermissionRequest(_)))
    );

    let pgid = handle.info().pid;
    handle.stop().await;
    assert_eq!(next(&mut handle).await, None);
    eventually("Claude Code's process group to empty", || {
        group_is_empty(pgid)
    })
    .await;
    assert_eq!(s.provider.unused(), 0);
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_asks_the_daemon_before_a_command_and_a_denial_stops_it() {
    let Some(s) = setup().await else { return };
    let ran = s.work.join("ran.txt");
    s.provider.reply(Reply::Tool {
        name: "Bash".into(),
        input: json!({ "command": format!("touch {}", ran.display()), "description": "touch" }),
    });
    s.provider.reply(Reply::Text("It was held.".into()));
    s.provider.reply(Reply::Tool {
        name: "mcp__x100__ping".into(),
        input: json!({ "msg": "hi" }),
    });
    s.provider.reply(Reply::Text("Pinged.".into()));
    let mut spec = s.spec(2);
    spec.permissions = Permissions::AskDaemon;
    spec.tools = Some(endpoint(&s.provider.mcp_url()));
    let mut handle = s.start(spec).await;

    handle.turn("Touch a file.").await.unwrap();
    let (_, asked) = until(&mut handle, |o| {
        matches!(o, DriverOutput::PermissionRequest(_))
    })
    .await;
    let DriverOutput::PermissionRequest(request) = asked else {
        unreachable!()
    };
    assert_eq!(request.tool, "Bash");
    handle
        .control()
        .answer(
            request.id,
            Decision::Deny {
                reason: "held for the builder".into(),
            },
        )
        .await
        .unwrap();
    let rest = until_end(&mut handle).await;
    assert!(rest.iter().any(|o| matches!(o, DriverOutput::ToolResult { output, is_error: true, .. } if output.contains("held for the builder"))), "{rest:#?}");
    assert_eq!(
        rest.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed))
    );
    assert!(!ran.exists(), "a denied command must not run");

    // The daemon's own tools never ask.
    handle.turn("Ping the daemon.").await.unwrap();
    let pinged = until_end(&mut handle).await;
    assert!(
        pinged
            .iter()
            .all(|o| !matches!(o, DriverOutput::PermissionRequest(_))),
        "{pinged:#?}"
    );
    handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_compacts_on_request_and_reports_the_context_left() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Text("Hello.".into()));
    s.provider
        .reply(Reply::Text("Summary: we said hello.".into()));
    let mut handle = s.start(s.spec(3)).await;
    handle.turn("Say hello.").await.unwrap();
    until_end(&mut handle).await;
    handle.control().compact().await.unwrap();
    let compacted = until_end(&mut handle).await;
    assert_eq!(
        compacted.last(),
        Some(&DriverOutput::Compacted),
        "{compacted:#?}"
    );
    assert!(
        compacted
            .iter()
            .any(|o| matches!(o, DriverOutput::Context(_))),
        "{compacted:#?}"
    );
    let summary_request = s.provider.requests().last().unwrap().body.to_string();
    assert!(
        summary_request.contains("Say hello."),
        "the summary request carries the history"
    );
    handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn stopping_claude_mid_command_leaves_nothing_running() {
    let Some(s) = setup().await else { return };
    let pid_file = s.work.join("pid.txt");
    s.provider.reply(Reply::Tool {
        name: "Bash".into(),
        input: json!({
            "command": format!("echo $$ > {}; exec sleep 300", pid_file.display()),
            "description": "wait",
        }),
    });
    let mut handle = s.start(s.spec(4)).await;
    let pgid = handle.info().pid;
    handle.turn("Wait a while.").await.unwrap();
    until(&mut handle, |o| matches!(o, DriverOutput::ToolCall { .. })).await;
    eventually("the command to start", || read_pid(&pid_file).is_some()).await;
    let sleeper = read_pid(&pid_file).unwrap();
    assert!(process_alive(sleeper));

    handle.stop().await;
    eventually("Claude Code's process group to empty", || {
        group_is_empty(pgid)
    })
    .await;
    eventually("the command Claude Code started to end", || {
        !process_alive(sleeper)
    })
    .await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_interrupted_mid_command_ends_the_turn_as_interrupted() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Tool {
        name: "Bash".into(),
        input: json!({ "command": "sleep 300", "description": "wait" }),
    });
    let mut handle = s.start(s.spec(5)).await;
    handle.turn("Wait a while.").await.unwrap();
    until(&mut handle, |o| matches!(o, DriverOutput::ToolCall { .. })).await;
    handle.control().interrupt().await.unwrap();
    let rest = until_end(&mut handle).await;
    assert_eq!(
        rest.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Interrupted)),
        "{rest:#?}"
    );
    handle.stop().await;
}

#[tokio::test]
async fn claude_reports_each_provider_error_as_the_right_provider_failure() {
    type Expected = fn(&ProviderFailure) -> bool;
    let cases: [(u16, Expected); 4] = [
        (401, |f| *f == ProviderFailure::KeyRefused),
        (429, |f| matches!(f, ProviderFailure::RateLimited { .. })),
        (500, |f| matches!(f, ProviderFailure::Outage { .. })),
        (529, |f| matches!(f, ProviderFailure::Outage { .. })),
    ];
    for (status, expected) in cases {
        let Some(s) = setup().await else { return };
        // The harness's own retries are set to one, but how many requests that makes differs by
        // status and harness, so every request it makes is answered with the error.
        for _ in 0..4 {
            s.provider.reply(Reply::Status(status));
        }
        let mut handle = s.start(s.spec(6)).await;
        let pgid = handle.info().pid;
        handle.turn("Say hello.").await.unwrap();
        let seen = until_end(&mut handle).await;
        match seen.last() {
            Some(DriverOutput::Failed(Failure::Error {
                provider: Some(provider),
                ..
            })) => assert!(expected(provider), "{status}: {provider:?}"),
            other => panic!("{status}: expected a provider failure, got {other:?}"),
        }
        eprintln!(
            "{status}: {} requests before failing",
            s.provider.requests().len()
        );
        eventually("the failed driver's group to empty", || {
            group_is_empty(pgid)
        })
        .await;
        s.assert_nothing_left_the_machine();
    }
}
