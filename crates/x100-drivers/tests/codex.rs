//! The real Codex binary, driven end to end against the fake provider on localhost. No request
//! leaves the machine: the provider is a custom one at the fake provider's URL with a dummy key,
//! the environment is built from nothing, and the proxy is the fake provider, which refuses and
//! records any attempt to reach another host.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use serde_json::json;
use x100_core::driver::{ContextFill, Failure};
use x100_core::effort::Effort;
use x100_core::harness::ProviderFailure;
use x100_core::ledger::CostBasis;
use x100_core::money::{Money, Usage};
use x100_core::time::{FakeClock, Timestamp};
use x100_drivers::codex::{Codex, CodexConfig, models};
use x100_drivers::testing::{FakeProvider, REPLY_CACHE_READ, REPLY_INPUT, REPLY_OUTPUT, Reply};
use x100_drivers::{
    Decision, DriverHandle, DriverOutput, DriverSpec, Harness, Permissions, TurnEnd, group_is_empty,
};

const KEY: &str = "sk-x100-fake-openai-key";
const MODEL: &str = "gpt-6-luna";

struct Setup {
    _dir: tempfile::TempDir,
    work: PathBuf,
    home: PathBuf,
    provider: FakeProvider,
    harness: Codex,
}

async fn setup() -> Option<Setup> {
    let program = installed("codex")?;
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let home = dir.path().join("home");
    for d in [&work, &home] {
        std::fs::create_dir(d).unwrap();
    }
    let provider = FakeProvider::start().await.unwrap();
    let mut config = CodexConfig::new();
    config.program = program;
    config.base_url = format!("{}/v1", provider.url());
    config.inherit_env = vec!["PATH".into()];
    let clock = FakeClock::new(Timestamp::from_millis(1));
    Some(Setup {
        _dir: dir,
        work,
        home,
        provider,
        harness: Codex::new(config, Arc::new(clock)),
    })
}

impl Setup {
    fn spec(&self, n: u128) -> DriverSpec {
        let mut spec = spec(n, &self.work, MODEL, Effort::Low);
        spec.env = vec![
            ("HOME".into(), self.home.display().to_string()),
            ("OPENAI_API_KEY".into(), KEY.into()),
            ("HTTPS_PROXY".into(), self.provider.url()),
            ("HTTP_PROXY".into(), self.provider.url()),
            ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
        ];
        spec
    }

    async fn start(&self, spec: DriverSpec) -> DriverHandle {
        let handle = self.harness.start(spec).await.unwrap();
        assert_eq!(handle.info().version, "0.167.1");
        assert!(
            handle.info().warnings.is_empty(),
            "{:?}",
            handle.info().warnings
        );
        handle
    }

    fn assert_nothing_left_the_machine(&self) {
        assert_eq!(self.provider.proxy_attempts(), Vec::<String>::new());
    }
}

fn patch(name: &str, line: &str) -> Reply {
    Reply::Freeform {
        name: "apply_patch".into(),
        input: format!("*** Begin Patch\n*** Add File: {name}\n+{line}\n*** End Patch\n"),
    }
}

fn warnings(outputs: &[DriverOutput]) -> Vec<&String> {
    outputs
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Warning(w) => Some(w),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn codex_edits_a_file_reports_usage_and_keeps_context_on_a_second_turn() {
    let Some(s) = setup().await else { return };
    s.provider.reply(patch("hello.txt", "hello from x100"));
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
    assert!(warnings(&first).is_empty(), "{first:#?}");
    assert_eq!(
        std::fs::read_to_string(s.work.join("hello.txt")).unwrap(),
        "hello from x100\n"
    );
    let calls: Vec<_> = first
        .iter()
        .filter(|o| matches!(o, DriverOutput::ToolCall { .. }))
        .collect();
    assert!(
        matches!(calls[..], [DriverOutput::ToolCall { name, .. }] if name == "file_change"),
        "{first:#?}"
    );
    assert!(first.iter().any(|o| matches!(
        o,
        DriverOutput::ToolResult {
            is_error: false,
            ..
        }
    )));
    assert!(first.contains(&DriverOutput::Text("Edited.".into())));
    // Each request reports its tokens; x100 prices each at the listed rate.
    let request = Usage {
        input: REPLY_INPUT,
        output: REPLY_OUTPUT,
        cache_read: REPLY_CACHE_READ,
        ..Usage::default()
    };
    let price = models()
        .into_iter()
        .find(|m| m.model.0 == MODEL)
        .unwrap()
        .levels[0]
        .1;
    let mut usage = request;
    usage += request;
    let spent: Vec<_> = first
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Spent(s) => Some(*s),
            _ => None,
        })
        .collect();
    assert_eq!(spent.len(), 1);
    assert_eq!(spent[0].usage, usage);
    assert_eq!(spent[0].cost, price.cost(&request) + price.cost(&request));
    assert!(spent[0].cost > Money::ZERO);
    assert_eq!(spent[0].basis, CostBasis::Listed);
    assert!(first.contains(&DriverOutput::Context(ContextFill {
        used: REPLY_INPUT + REPLY_CACHE_READ + REPLY_OUTPUT,
        window: 258_400,
    })));
    assert_eq!(
        first.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed))
    );

    let requests = s.provider.requests();
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(requests[0].body["reasoning"]["effort"], "low");
    assert!(
        requests[0].body["input"]
            .to_string()
            .contains("START-PACK-MARKER")
    );

    // A second turn on the same thread, calling one of the daemon's tools through Codex's
    // JavaScript `exec` tool, which is how gpt-6-luna reaches MCP tools.
    s.provider.reply(Reply::Freeform {
        name: "exec".into(),
        input: "const r = await tools.mcp__x100__ping({ msg: \"hi\" }); text(JSON.stringify(r));"
            .into(),
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
    assert!(
        second
            .iter()
            .any(|o| matches!(o, DriverOutput::ToolCall { name, .. } if name == "mcp__x100__ping")),
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

    let pgid = handle.info().pid;
    handle.stop().await;
    assert_eq!(next(&mut handle).await, None);
    eventually("Codex's process group to empty", || group_is_empty(pgid)).await;
    assert_eq!(s.provider.unused(), 0);
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_asks_the_daemon_before_a_command_and_a_denial_stops_it() {
    let Some(s) = setup().await else { return };
    let ran = s.work.join("ran.txt");
    s.provider.reply(Reply::Tool {
        name: "exec_command".into(),
        input: json!({ "cmd": format!("touch {}", ran.display()) }),
    });
    s.provider.reply(Reply::Text("It was held.".into()));
    let mut spec = s.spec(2);
    spec.permissions = Permissions::AskDaemon;
    let mut handle = s.start(spec).await;

    handle.turn("Touch a file.").await.unwrap();
    let (_, asked) = until(&mut handle, |o| {
        matches!(o, DriverOutput::PermissionRequest(_))
    })
    .await;
    let DriverOutput::PermissionRequest(request) = asked else {
        unreachable!()
    };
    assert_eq!(request.tool, "command");
    assert!(
        request.input["command"].to_string().contains("touch"),
        "{request:?}"
    );
    handle
        .control()
        .answer(
            request.id,
            Decision::Deny {
                reason: "held".into(),
            },
        )
        .await
        .unwrap();
    let rest = until_end(&mut handle).await;
    assert!(
        rest.iter()
            .any(|o| matches!(o, DriverOutput::ToolResult { is_error: true, .. })),
        "{rest:#?}"
    );
    assert_eq!(
        rest.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed)),
        "{rest:#?}"
    );
    assert!(!ran.exists(), "a denied command must not run");
    handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_compacts_on_request() {
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
    let summary_request = s.provider.requests().last().unwrap().body.to_string();
    assert!(
        summary_request.contains("Say hello."),
        "the summary request carries the history"
    );
    handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn stopping_codex_mid_command_leaves_nothing_running() {
    let Some(s) = setup().await else { return };
    let pid_file = s.work.join("pid.txt");
    s.provider.reply(Reply::Tool {
        name: "exec_command".into(),
        input: json!({ "cmd": format!("echo $$ > {}; exec sleep 300", pid_file.display()) }),
    });
    let mut handle = s.start(s.spec(4)).await;
    let pgid = handle.info().pid;
    handle.turn("Wait a while.").await.unwrap();
    until(&mut handle, |o| matches!(o, DriverOutput::ToolCall { .. })).await;
    eventually("the command to start", || read_pid(&pid_file).is_some()).await;
    let sleeper = read_pid(&pid_file).unwrap();
    assert!(process_alive(sleeper));

    handle.stop().await;
    eventually("Codex's process group to empty", || group_is_empty(pgid)).await;
    eventually("the command Codex started to end", || {
        !process_alive(sleeper)
    })
    .await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_interrupted_mid_command_ends_the_turn_as_interrupted() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Tool {
        name: "exec_command".into(),
        input: json!({ "cmd": "sleep 300", "yield_time_ms": 600000 }),
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
async fn codex_reports_each_provider_error_as_the_right_provider_failure() {
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
            other => {
                panic!("{status}: expected a provider failure, got {seen:#?} ending {other:?}")
            }
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
