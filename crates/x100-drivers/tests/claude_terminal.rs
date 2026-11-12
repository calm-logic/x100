//! The real `claude` program in a pseudo-terminal, driven end to end against the fake provider on
//! localhost. The login is a dummy subscription login written here into a throwaway directory that
//! stands in for the builder's `~/.claude`; the base URL is the fake provider; the environment is
//! built from nothing; and the proxy is the fake provider, which refuses and records any attempt to
//! reach another host.

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
use x100_drivers::claude::{ClaudeTerminal, ClaudeTerminalConfig, models};
use x100_drivers::terminal::Size;
use x100_drivers::testing::{
    FakeProvider, REPLY_CACHE_READ, REPLY_CACHE_WRITE, REPLY_INPUT, REPLY_OUTPUT, Reply,
};
use x100_drivers::{
    Decision, DriverOutput, DriverSpec, Harness, Permissions, TurnEnd, group_is_empty,
};

const MODEL: &str = "claude-haiku-5-5";
const ACCESS: &str = "sk-ant-oat01-x100-dummy-access-token";

struct Setup {
    _dir: tempfile::TempDir,
    work: PathBuf,
    login: PathBuf,
    provider: FakeProvider,
    harness: ClaudeTerminal,
}

async fn setup() -> Option<Setup> {
    let program = installed("claude")?;
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let login = dir.path().join("login");
    for d in [&work, &login] {
        std::fs::create_dir(d).unwrap();
    }
    // A subscription login as Claude Code writes it, good for hours.
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 8 * 3600 * 1000;
    let credentials = json!({ "claudeAiOauth": {
        "accessToken": ACCESS, "refreshToken": "sk-ant-ort01-x100-dummy-refresh",
        "expiresAt": expires as u64,
        "scopes": ["user:inference", "user:profile", "user:sessions:claude_code"],
        "subscriptionType": "max",
    }});
    std::fs::write(login.join(".credentials.json"), credentials.to_string()).unwrap();
    let provider = FakeProvider::start().await.unwrap();
    let mut config = ClaudeTerminalConfig::new();
    config.program = program;
    config.base_url = Some(provider.url());
    config.inherit_env = vec!["PATH".into()];
    let clock = FakeClock::new(Timestamp::from_millis(1));
    Some(Setup {
        _dir: dir,
        work,
        login,
        provider,
        harness: ClaudeTerminal::new(config, Arc::new(clock)),
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
        assert_eq!(handle.info().version, "2.1.322");
        assert!(
            handle.info().warnings.is_empty(),
            "{:?}",
            handle.info().warnings
        );
        Watched::new(handle)
    }

    /// On a subscription login Claude Code fetches the account's profile from its OAuth host,
    /// which the base URL does not move. The fake provider, as the proxy, refuses it, and the
    /// driver works without it. Nothing else is tried.
    fn assert_nothing_left_the_machine(&self) {
        self.assert_only_tried(&["api.anthropic.com:443"]);
    }

    fn assert_only_tried(&self, hosts: &[&str]) {
        let attempts = self.provider.proxy_attempts();
        assert!(
            attempts.iter().all(|a| hosts.contains(&a.as_str())),
            "{attempts:?}"
        );
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
async fn claude_in_a_terminal_edits_calls_a_tool_keeps_context_and_replays() {
    let Some(s) = setup().await else { return };
    assert_eq!(s.harness.view(), View::Terminal);
    let file = s.work.join("hello.txt");
    s.provider.reply(Reply::Tool {
        name: "Write".into(),
        input: json!({ "file_path": file, "content": "hello from x100\n" }),
    });
    s.provider.reply(Reply::Tool {
        name: "mcp__x100__ping".into(),
        input: json!({ "msg": "hi" }),
    });
    s.provider.reply(Reply::Text("Done one.".into()));
    let mut spec = s.spec(1);
    spec.start_pack = "You are the x100 test agent. START-PACK-MARKER".into();
    spec.tools = Some(endpoint(&s.provider.mcp_url()));
    let mut w = s.start(spec).await;
    assert!(w.handle.info().session.is_some());

    w.handle
        .turn("Create hello.txt and ping. FIRST-TURN-MARKER")
        .await
        .unwrap();
    let first = w
        .until(|o| matches!(o, DriverOutput::ToolCall { name, .. } if name == "Write"))
        .await;
    let chunks_at_call = w.chunks.len();
    let mut first = [first, w.until_end().await].concat();
    assert!(
        w.chunks.len() > chunks_at_call,
        "the terminal is followed during the turn, not only at its end"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello from x100\n");
    let calls: Vec<_> = first
        .iter()
        .filter_map(|o| match o {
            DriverOutput::ToolCall { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["Write", "mcp__x100__ping"], "{first:#?}");
    assert!(
        first.iter().any(|o| matches!(o, DriverOutput::ToolResult { output, is_error: false, .. } if output.contains("pong"))),
        "{first:#?}"
    );
    assert!(
        first.contains(&DriverOutput::Text("Done one.".into())),
        "{first:#?}"
    );
    assert!(
        matches!(first[first.len() - 2], DriverOutput::Context(c) if c.window == 1_000_000 && c.used > 0),
        "{first:#?}"
    );
    assert_eq!(
        first.pop(),
        Some(DriverOutput::TurnEnded(TurnEnd::Completed))
    );
    // Three model requests, each with the fake provider's tokens, at Haiku's listed price.
    let usage = Usage {
        input: 3 * REPLY_INPUT,
        output: 3 * REPLY_OUTPUT,
        cache_read: 3 * REPLY_CACHE_READ,
        cache_write_short: 3 * REPLY_CACHE_WRITE,
        cache_write_long: 0,
    };
    let turn_spent = spent(&first);
    assert_eq!(turn_spent.len(), 1);
    assert_eq!(turn_spent[0].usage, usage);
    assert_eq!(turn_spent[0].basis, CostBasis::Listed);
    // Each request is priced on its own, since the long-prompt tier applies per request.
    let listed = models()
        .into_iter()
        .find(|m| m.model.0 == MODEL)
        .unwrap()
        .levels[0]
        .1;
    let one = Usage {
        input: REPLY_INPUT,
        output: REPLY_OUTPUT,
        cache_read: REPLY_CACHE_READ,
        cache_write_short: REPLY_CACHE_WRITE,
        cache_write_long: 0,
    };
    let per_request = listed.cost(&one);
    assert_eq!(turn_spent[0].cost, per_request + per_request + per_request);

    let requests = s.provider.requests();
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {ACCESS}").as_str()),
        "the request carries the login Claude Code read itself"
    );
    assert!(
        requests[0].body["system"]
            .to_string()
            .contains("START-PACK-MARKER")
    );
    assert!(
        s.provider
            .mcp_calls()
            .iter()
            .any(|c| c.method == "tools/call"
                && c.authorization.as_deref() == Some("Bearer drv-token-123"))
    );

    // A resize in the middle is recorded where the program starts to follow it.
    w.handle
        .control()
        .resize(Size {
            cols: 100,
            rows: 30,
        })
        .await
        .unwrap();

    // The second turn is typed into the same program and sends the first back to the model.
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
    assert_eq!(
        second.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed))
    );
    let last = s.provider.requests().last().unwrap().body.to_string();
    for earlier in ["FIRST-TURN-MARKER", "Done one.", "SECOND-TURN-MARKER"] {
        assert!(
            last.contains(earlier),
            "the second turn's request lacks {earlier}"
        );
    }
    w.until_screen_shows("Second turn.").await;
    w.assert_replay_matches().await;
    assert!(w.chunks.iter().any(|c| matches!(
        c.what,
        x100_core::terminal::ChunkKind::Resize {
            cols: 100,
            rows: 30
        }
    )));

    let pgid = w.handle.info().pid;
    w.handle.stop().await;
    while w.next().await.is_some() {}
    eventually("Claude Code's process group to empty", || {
        group_is_empty(pgid)
    })
    .await;
    assert_eq!(s.provider.unused(), 0);
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_in_a_terminal_holds_a_command_for_the_daemon() {
    let Some(s) = setup().await else { return };
    let ran = s.work.join("ran.txt");
    let command = format!("touch {}", ran.display());
    s.provider.reply(Reply::Tool {
        name: "Bash".into(),
        input: json!({ "command": command, "description": "touch" }),
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
    assert_eq!(request.tool, "Bash");
    assert_eq!(request.input["command"], json!(command));
    // The turn cannot end while the command is held, so what the terminal shows now arrived
    // while the turn was running.
    w.until_screen_shows("touch").await;
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
    let last = s.provider.requests().last().unwrap().body.to_string();
    assert!(
        last.contains("held for the builder"),
        "the model is told why"
    );
    w.handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_in_a_terminal_compacts_on_request() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Text("Hello.".into()));
    s.provider.reply(Reply::Text(
        "<analysis>Short.</analysis><summary>We said hello.</summary>".into(),
    ));
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
    let summary_request = s.provider.requests().last().unwrap().body.to_string();
    assert!(
        summary_request.contains("Say hello."),
        "the summary request carries the history"
    );
    w.handle.stop().await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_in_a_terminal_interrupted_mid_command_ends_the_turn() {
    let Some(s) = setup().await else { return };
    s.provider.reply(Reply::Tool {
        name: "Bash".into(),
        // Claude Code refuses a bare `sleep` and suggests its own waiting tools.
        input: json!({ "command": "python3 -c 'import time; time.sleep(300)'", "description": "wait" }),
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
async fn stopping_claude_in_a_terminal_leaves_nothing_running() {
    let Some(s) = setup().await else { return };
    let pid_file = s.work.join("pid.txt");
    // A background command runs in a session of its own, outside Claude Code's process group.
    s.provider.reply(Reply::Tool {
        name: "Bash".into(),
        input: json!({
            "command": format!("echo $$ > {}; exec sleep 300", pid_file.display()),
            "description": "wait",
            "run_in_background": true,
        }),
    });
    s.provider.reply(Reply::Text("Started.".into()));
    let mut w = s.start(s.spec(5)).await;
    let pgid = w.handle.info().pid;
    w.handle.turn("Start a background job.").await.unwrap();
    w.until_end().await;
    eventually("the command to start", || read_pid(&pid_file).is_some()).await;
    let sleeper = read_pid(&pid_file).unwrap();
    assert!(process_alive(sleeper));

    w.handle.stop().await;
    eventually("Claude Code's process group to empty", || {
        group_is_empty(pgid)
    })
    .await;
    eventually("the background command to end", || !process_alive(sleeper)).await;
    s.assert_nothing_left_the_machine();
}

#[tokio::test]
async fn claude_in_a_terminal_reports_each_provider_error_as_the_right_failure() {
    type Expected = fn(&ProviderFailure) -> bool;
    let cases: [(u16, Expected); 4] = [
        (401, |f| *f == ProviderFailure::KeyRefused),
        (429, |f| matches!(f, ProviderFailure::RateLimited { .. })),
        (500, |f| matches!(f, ProviderFailure::Outage { .. })),
        (529, |f| matches!(f, ProviderFailure::Outage { .. })),
    ];
    for (status, expected) in cases {
        let Some(s) = setup().await else { return };
        for _ in 0..4 {
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
        eventually("the failed driver's group to empty", || {
            group_is_empty(pgid)
        })
        .await;
        if status == 401 {
            // A refused subscription token is refreshed at Anthropic's OAuth host first.
            s.assert_only_tried(&["api.anthropic.com:443", "platform.claude.com:443"]);
        } else {
            s.assert_nothing_left_the_machine();
        }
    }
}

#[tokio::test]
async fn claude_in_a_terminal_loads_nothing_of_the_builders_setup() {
    let Some(s) = setup().await else { return };
    // The builder's own setup, in the login's directory, as Claude Code would find it.
    let marker = s.work.join("BUILDER-HOOK-RAN");
    let builder_settings = json!({
        "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command",
            "command": format!("touch {}", marker.display()) }] }] },
        "env": { "BUILDER_ENV": "MARKER-BUILDER-ENV" },
    });
    let files = [
        ("settings.json", builder_settings.to_string()),
        ("CLAUDE.md", "MARKER-BUILDER-CLAUDE-MD\n".into()),
        (
            "skills/bskill/SKILL.md",
            "---\nname: bskill\ndescription: MARKER-BUILDER-SKILL\n---\nbody\n".into(),
        ),
        (
            "agents/bagent.md",
            "---\nname: bagent\ndescription: MARKER-BUILDER-AGENT\n---\nbody\n".into(),
        ),
        (
            "commands/bcmd.md",
            "---\ndescription: MARKER-BUILDER-COMMAND\n---\nbody\n".into(),
        ),
    ];
    for (name, content) in &files {
        let path = s.login.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let before = files_under(&s.login);
    let credentials = std::fs::read(s.login.join(".credentials.json")).unwrap();
    s.provider.reply(Reply::Text("Hello.".into()));
    let mut w = s.start(s.spec(7)).await;
    w.handle.turn("Say hello.").await.unwrap();
    w.until_end().await;
    w.handle.stop().await;

    let request = s.provider.requests()[0].body.to_string();
    let tools: Vec<_> = s.provider.requests()[0].body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        !tools.iter().any(|t| t == "AskUserQuestion"),
        "nobody answers a question asked on the terminal: {tools:?}"
    );
    for marker in [
        "MARKER-BUILDER-ENV",
        "MARKER-BUILDER-CLAUDE-MD",
        "MARKER-BUILDER-SKILL",
        "MARKER-BUILDER-AGENT",
        "MARKER-BUILDER-COMMAND",
    ] {
        assert!(
            !request.contains(marker),
            "the builder's {marker} reached the model"
        );
    }
    assert!(!marker.exists(), "the builder's hook ran");
    assert_eq!(
        std::fs::read(s.login.join(".credentials.json")).unwrap(),
        credentials,
        "the login is left as it was"
    );
    let added: Vec<_> = files_under(&s.login)
        .into_iter()
        .filter(|f| !before.contains(f))
        .collect();
    eprintln!("written into the login's directory: {added:#?}");
    for file in &added {
        let top = file
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        assert!(
            [
                "projects",
                "sessions",
                "session-env",
                "file-history",
                "shell-snapshots",
                "backups",
                "history.jsonl",
                "plugins"
            ]
            .contains(&top.as_str()),
            "unexpected file written into the login's directory: {file:?}"
        );
    }
    s.assert_nothing_left_the_machine();
}
