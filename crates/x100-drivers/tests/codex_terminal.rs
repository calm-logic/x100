//! The real `codex` app-server and `codex --remote` terminal, driven end to end against the fake
//! provider on localhost. The login is a dummy `auth.json` written here into a throwaway directory
//! that stands in for the builder's `~/.codex`; OpenAI's base URL is the fake provider; the
//! environment is built from nothing; and the proxy is the fake provider, which refuses and
//! records any attempt to reach another host.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::terminal::{Watched, files_under};
use common::*;
use serde_json::json;
use x100_core::driver::{Failure, View};
use x100_core::effort::Effort;
use x100_core::harness::ProviderFailure;
use x100_core::ledger::CostBasis;
use x100_core::money::Usage;
use x100_core::time::{FakeClock, Timestamp};
use x100_drivers::codex::{CodexTerminal, CodexTerminalConfig, models};
use x100_drivers::testing::{FakeProvider, REPLY_CACHE_READ, REPLY_INPUT, REPLY_OUTPUT, Reply};
use x100_drivers::{
    Decision, DriverOutput, DriverSpec, Harness, Permissions, TurnEnd, group_is_empty,
};

const MODEL: &str = "gpt-6-luna";
const KEY: &str = "sk-x100-dummy-login-key";

struct Setup {
    _dir: tempfile::TempDir,
    work: PathBuf,
    login: PathBuf,
    provider: FakeProvider,
    harness: CodexTerminal,
}

async fn setup() -> Option<Setup> {
    let program = installed("codex")?;
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let login = dir.path().join("login");
    for d in [&work, &login] {
        std::fs::create_dir(d).unwrap();
    }
    let auth = json!({ "auth_mode": "apikey", "OPENAI_API_KEY": KEY });
    std::fs::write(login.join("auth.json"), auth.to_string()).unwrap();
    let provider = FakeProvider::start().await.unwrap();
    let mut config = CodexTerminalConfig::new();
    config.program = program;
    config.base_url = Some(format!("{}/v1", provider.url()));
    config.inherit_env = vec!["PATH".into()];
    let clock = FakeClock::new(Timestamp::from_millis(1));
    Some(Setup {
        _dir: dir,
        work,
        login,
        provider,
        harness: CodexTerminal::new(config, Arc::new(clock)),
    })
}

impl Setup {
    fn spec(&self, n: u128) -> DriverSpec {
        let mut spec = spec(n, &self.work, MODEL, Effort::Low);
        spec.login = Some(self.login.clone());
        spec.env = vec![
            ("HTTPS_PROXY".into(), self.provider.url()),
            ("HTTP_PROXY".into(), self.provider.url()),
            ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
        ];
        spec
    }

    async fn start(&self, spec: DriverSpec) -> Watched {
        let handle = self.harness.start(spec).await.unwrap();
        // Codex is updated on this machine from time to time, so the installed version is whatever
        // it has. A version other than the tested one is allowed its one warning, and the tests
        // that follow say whether the newer program still behaves.
        let other: Vec<_> = handle
            .info()
            .warnings
            .iter()
            .filter(|w| !w.contains("is not the version x100 was tested with"))
            .collect();
        assert!(other.is_empty(), "{other:?}");
        Watched::new(handle)
    }

    /// Codex's terminal fetches an announcement tip from GitHub at every start, with no setting
    /// to stop it. The fake provider, as the proxy, refuses it. Nothing else is tried.
    fn assert_nothing_left_the_machine(&self) {
        let attempts = self.provider.proxy_attempts();
        assert!(
            attempts
                .iter()
                .all(|a| a == "raw.githubusercontent.com:443"),
            "{attempts:?}"
        );
    }
}

fn patch(name: &str, line: &str) -> Reply {
    Reply::Freeform {
        name: "apply_patch".into(),
        input: format!("*** Begin Patch\n*** Add File: {name}\n+{line}\n*** End Patch\n"),
    }
}

/// Codex on gpt-6-luna reaches MCP tools through its `exec` tool, which runs JavaScript.
fn ping() -> Reply {
    Reply::Freeform {
        name: "exec".into(),
        input: "const r = await tools.mcp__x100__ping({ msg: \"hi\" }); text(JSON.stringify(r));"
            .into(),
    }
}

#[tokio::test]
async fn codex_in_a_terminal_edits_calls_a_tool_keeps_context_and_replays() {
    let Some(s) = setup().await else { return };
    assert_eq!(s.harness.view(), View::Terminal);
    s.provider.reply(patch("hello.txt", "hello from x100"));
    s.provider.reply(ping());
    s.provider.reply(Reply::Text("Done one.".into()));
    let mut spec = s.spec(1);
    spec.start_pack = "You are the x100 test agent. START-PACK-MARKER".into();
    spec.tools = Some(endpoint(&s.provider.mcp_url()));
    let mut w = s.start(spec).await;
    assert!(w.handle.info().session.is_some(), "the thread's id");

    w.handle
        .turn("Create hello.txt and ping. FIRST-TURN-MARKER")
        .await
        .unwrap();
    let first = w.until_end().await;
    assert_eq!(
        std::fs::read_to_string(s.work.join("hello.txt")).unwrap(),
        "hello from x100\n"
    );
    assert!(
        first
            .iter()
            .any(|o| matches!(o, DriverOutput::ToolCall { name, .. } if name == "file_change")),
        "{first:#?}"
    );
    assert!(
        first.contains(&DriverOutput::Text("Done one.".into())),
        "{first:#?}"
    );
    assert_eq!(
        first.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed))
    );
    let spent: Vec<_> = first
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Spent(s) => Some(*s),
            _ => None,
        })
        .collect();
    assert_eq!(spent.len(), 1);
    let one = Usage {
        input: REPLY_INPUT,
        output: REPLY_OUTPUT,
        cache_read: REPLY_CACHE_READ,
        cache_write_short: 0,
        cache_write_long: 0,
    };
    assert_eq!(spent[0].usage.output, 3 * one.output, "{spent:?}");
    assert_eq!(spent[0].basis, CostBasis::Listed);
    let listed = models()
        .into_iter()
        .find(|m| m.model.0 == MODEL)
        .unwrap()
        .levels[0]
        .1;
    assert!(spent[0].cost >= listed.cost(&one), "{spent:?}");

    let requests = s.provider.requests();
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str()),
        "the request carries the login Codex read itself"
    );
    assert!(requests[0].body.to_string().contains("START-PACK-MARKER"));
    assert!(
        s.provider
            .mcp_calls()
            .iter()
            .any(|c| c.method == "tools/call"
                && c.authorization.as_deref() == Some("Bearer drv-token-123"))
    );

    s.provider.reply(Reply::Text("Second turn.".into()));
    w.handle
        .turn("What did you do? SECOND-TURN-MARKER")
        .await
        .unwrap();
    let second = w.until_end().await;
    assert!(
        second.contains(&DriverOutput::Text("Second turn.".into())),
        "{second:#?}"
    );
    let last = s.provider.requests().last().unwrap().body.to_string();
    for earlier in ["FIRST-TURN-MARKER", "Done one.", "SECOND-TURN-MARKER"] {
        assert!(
            last.contains(earlier),
            "the second turn's request lacks {earlier}"
        );
    }
    // The terminal attached to the daemon's thread draws the daemon's turns, at every size.
    w.handle
        .control()
        .resize(x100_drivers::terminal::Size { cols: 90, rows: 30 })
        .await
        .unwrap();
    w.until_screen_shows("Second turn.").await;
    w.assert_replay_matches().await;

    let pgid = w.handle.info().pid;
    w.handle.stop().await;
    while w.next().await.is_some() {}
    eventually("the app-server's process group to empty", || {
        group_is_empty(pgid)
    })
    .await;
    assert_eq!(s.provider.unused(), 0, "no request beyond the turns'");
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_in_a_terminal_holds_a_command_for_the_daemon() {
    let Some(s) = setup().await else { return };
    let ran = s.work.join("ran.txt");
    let command = format!("touch {}", ran.display());
    s.provider.reply(Reply::Tool {
        name: "exec_command".into(),
        input: json!({ "cmd": command }),
    });
    s.provider.reply(Reply::Text("It was held.".into()));
    let mut spec = s.spec(2);
    spec.permissions = Permissions::AskDaemon;
    let mut w = s.start(spec).await;

    w.handle.turn("Touch a file.").await.unwrap();
    let seen = w
        .until(|o| matches!(o, DriverOutput::PermissionRequest(_)))
        .await;
    let Some(DriverOutput::PermissionRequest(request)) = seen.last().cloned() else {
        unreachable!()
    };
    assert_eq!(request.tool, "command");
    // The turn cannot end while the command is held, so what the terminal shows now arrived
    // while the turn was running.
    w.until_screen_shows("Touch a file.").await;
    w.handle
        .control()
        .answer(
            request.id,
            Decision::Deny {
                reason: "held for the builder".into(),
            },
        )
        .await
        .unwrap();
    let rest = w.until_end().await;
    assert_eq!(
        rest.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed)),
        "{rest:#?}"
    );
    assert!(!ran.exists(), "a denied command must not run");
    w.handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_in_a_terminal_compacts_on_request() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Text("Hello.".into()));
    // On an OpenAI login Codex compacts through OpenAI, which answers with an opaque item.
    s.provider
        .reply(Reply::Compaction("SUMMARY-OF-HELLO".into()));
    s.provider.reply(Reply::Text("Still here.".into()));
    let mut w = s.start(s.spec(3)).await;
    w.handle.turn("Say hello.").await.unwrap();
    w.until_end().await;
    w.handle.control().compact().await.unwrap();
    let compacted = w.until_end().await;
    assert_eq!(
        compacted.last(),
        Some(&DriverOutput::Compacted),
        "{compacted:#?}"
    );
    // The next turn starts from the compacted context.
    w.handle.turn("Are you there?").await.unwrap();
    w.until_end().await;
    let next = s.provider.requests().last().unwrap().body.to_string();
    assert!(
        next.contains("SUMMARY-OF-HELLO"),
        "the next request carries the compaction"
    );
    w.handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_in_a_terminal_interrupted_mid_command_ends_the_turn() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Tool {
        name: "exec_command".into(),
        input: json!({ "cmd": "sleep 300" }),
    });
    let mut w = s.start(s.spec(4)).await;
    w.handle.turn("Wait a while.").await.unwrap();
    w.until(|o| matches!(o, DriverOutput::ToolCall { .. }))
        .await;
    w.handle.control().interrupt().await.unwrap();
    let rest = w.until_end().await;
    assert_eq!(
        rest.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Interrupted)),
        "{rest:#?}"
    );
    w.handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn stopping_codex_in_a_terminal_leaves_nothing_running() {
    let Some(s) = setup().await else { return };
    let pid_file = s.work.join("pid.txt");
    s.provider.reply(Reply::Tool {
        name: "exec_command".into(),
        input: json!({ "cmd": format!("echo $$ > {}; exec sleep 300", pid_file.display()) }),
    });
    let spawned = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut spec = s.spec(5);
    spec.on_spawn = x100_drivers::SpawnHook::new({
        let spawned = Arc::clone(&spawned);
        move |pid| spawned.lock().unwrap().push(pid)
    });
    let mut w = s.start(spec).await;
    let pids = spawned.lock().unwrap().clone();
    assert_eq!(pids.len(), 2, "the app-server and the terminal");
    w.handle.turn("Wait a while.").await.unwrap();
    w.until(|o| matches!(o, DriverOutput::ToolCall { .. }))
        .await;
    eventually("the command to start", || read_pid(&pid_file).is_some()).await;
    let sleeper = read_pid(&pid_file).unwrap();
    assert!(process_alive(sleeper));

    w.handle.stop().await;
    for pid in pids {
        eventually("each of the driver's process groups to empty", || {
            group_is_empty(pid)
        })
        .await;
    }
    eventually("the command Codex started to end", || {
        !process_alive(sleeper)
    })
    .await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn codex_in_a_terminal_reports_each_provider_error_as_the_right_failure() {
    type Expected = fn(&ProviderFailure) -> bool;
    let cases: [(u16, Expected); 4] = [
        (401, |f| *f == ProviderFailure::KeyRefused),
        (429, |f| matches!(f, ProviderFailure::RateLimited { .. })),
        (500, |f| matches!(f, ProviderFailure::Outage { .. })),
        (529, |f| matches!(f, ProviderFailure::Outage { .. })),
    ];
    for (status, expected) in cases {
        let Some(s) = setup().await else { return };
        for _ in 0..8 {
            s.provider.reply(Reply::Status(status));
        }
        let mut w = s.start(s.spec(6)).await;
        let pgid = w.handle.info().pid;
        w.handle.turn("Say hello.").await.unwrap();
        let seen = w.until_end().await;
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

#[tokio::test]
async fn codex_in_a_terminal_loads_nothing_of_the_builders_setup_and_writes_nothing_there() {
    let Some(s) = setup().await else { return };
    let marker = s.work.join("BUILDER-NOTIFY-RAN");
    let builder_config = format!(
        "model = \"gpt-builder\"\n\
         developer_instructions = \"MARKER-BUILDER-DEV\"\n\
         notify = [\"touch\", \"{}\"]\n\
         [mcp_servers.builder]\nurl = \"http://127.0.0.1:9/mcp\"\n",
        marker.display()
    );
    let files = [
        ("config.toml", builder_config),
        ("AGENTS.md", "MARKER-BUILDER-AGENTS\n".to_owned()),
        (
            "skills/bskill/SKILL.md",
            "---\nname: bskill\ndescription: MARKER-BUILDER-SKILL\n---\nbody\n".to_owned(),
        ),
    ];
    for (name, content) in &files {
        let path = s.login.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let before = files_under(&s.login);
    let auth = std::fs::read(s.login.join("auth.json")).unwrap();
    s.provider.reply(Reply::Text("Hello.".into()));
    let mut w = s.start(s.spec(7)).await;
    w.handle.turn("Say hello.").await.unwrap();
    w.until_end().await;
    w.handle.stop().await;

    let request = &s.provider.requests()[0].body;
    assert_eq!(request["model"], MODEL);
    let text = request.to_string();
    for marker in [
        "MARKER-BUILDER-DEV",
        "MARKER-BUILDER-AGENTS",
        "MARKER-BUILDER-SKILL",
    ] {
        assert!(
            !text.contains(marker),
            "the builder's {marker} reached the model"
        );
    }
    assert!(!marker.exists(), "the builder's notify command ran");
    assert_eq!(std::fs::read(s.login.join("auth.json")).unwrap(), auth);
    assert_eq!(
        files_under(&s.login),
        before,
        "nothing is written into the builder's directory"
    );
    s.assert_nothing_left_the_machine();
}
