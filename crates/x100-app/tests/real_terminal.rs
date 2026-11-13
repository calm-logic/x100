//! The CEO's view in the app, with no display, against the real daemon running the real `claude`
//! program as a terminal driver: what the view shows is the driver's screen, replayed at the sizes
//! the daemon recorded, the same as a bare emulator shows for the same chunks. The project is new:
//! the CEO sets it up on a driver in the scratch directory, and the work is done on a second
//! driver in the worktree, whose screen is the one checked.
//!
//! No request leaves the machine and no real login is touched: the login is a dummy in a throwaway
//! directory the daemon is pointed at, the daemon's home and environment are throwaway, and the
//! model, the base URL and the proxy are the fake provider on localhost, which refuses and records
//! any attempt to reach another host. Skipped, saying so, when `claude` is not installed.

mod common;

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use gpui::{TestAppContext, prelude::*};
use serde_json::json;
use x100_app::session::Session;
use x100_app::team::TeamView;
use x100_app::term::{Size, screen_text};
use x100_client::{Client, Options};
use x100_core::action::AskMeAbout;
use x100_core::agent::{AgentState, Waiting};
use x100_core::driver::DriverState;
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::id::{DriverId, ProjectId};
use x100_core::money::{Money, Percent};
use x100_core::project::{SettingChange, Settings};
use x100_core::settings::DaemonSetting;
use x100_core::task::TaskState;
use x100_core::terminal::{Chunk, ChunkKind, TermPos};
use x100_drivers::testing::{FakeProvider, Reply};
use x100_proto::{CreateProject, ReadTerminal, SetDaemonSetting};

const PATIENCE: Duration = Duration::from_secs(120);

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn claude_installed() -> bool {
    let found = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("claude").is_file()));
    if !found {
        eprintln!("SKIPPED: claude is not installed on this machine");
    }
    found
}

/// A subscription login as Claude Code writes it, with dummy tokens.
fn dummy_login(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 8 * 3600 * 1000;
    let credentials = json!({ "claudeAiOauth": {
        "accessToken": "sk-ant-oat01-x100-dummy-access-token",
        "refreshToken": "sk-ant-ort01-x100-dummy-refresh",
        "expiresAt": expires,
        "scopes": ["user:inference", "user:profile", "user:sessions:claude_code"],
        "subscriptionType": "max",
    }});
    std::fs::write(dir.join(".credentials.json"), credentials.to_string()).unwrap();
}

/// The chunks replayed into a bare emulator, resized where a chunk says so.
fn bare_replay(chunks: &[(TermPos, Chunk)]) -> Vec<String> {
    let size = |cols: u16, rows: u16| Size {
        cols: cols.into(),
        rows: rows.into(),
    };
    let ChunkKind::Resize { cols, rows } = chunks[0].1.what else {
        panic!("a terminal starts with its size");
    };
    let mut term = Term::new(Config::default(), &size(cols, rows), VoidListener);
    let mut parser: Processor = Processor::new();
    for (_, chunk) in chunks {
        match &chunk.what {
            ChunkKind::Resize { cols, rows } => term.resize(size(*cols, *rows)),
            ChunkKind::Output { bytes } => parser.advance(&mut term, &bytes.0),
        }
    }
    screen_text(&term)
}

async fn read_all(client: &Client, project: ProjectId, driver: DriverId) -> Vec<(TermPos, Chunk)> {
    let mut out: Vec<(TermPos, Chunk)> = Vec::new();
    loop {
        let after = out.last().map(|(p, _)| *p);
        let page = client
            .request(ReadTerminal {
                project,
                driver,
                after,
                limit: 1000,
            })
            .await
            .unwrap();
        if page.chunks.is_empty() {
            return out;
        }
        out.extend(page.chunks.into_iter().map(|c| (c.position, c.chunk)));
    }
}

#[test]
fn the_ceos_view_shows_its_terminal_drivers_screen_as_recorded() {
    if !claude_installed() || common::available::skip_without_sandbox() {
        return;
    }
    let mut cx = TestAppContext::single();
    cx.update(x100_app::init);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (data, home, login) = (
        dir.path().join("data"),
        dir.path().join("home"),
        dir.path().join("claude-login"),
    );
    std::fs::create_dir_all(&home).unwrap();
    dummy_login(&login);
    let provider = rt.block_on(FakeProvider::start()).unwrap();
    let child = Command::new(common::binary("x100d"))
        .arg("--data-dir")
        .arg(&data)
        .arg("--projects-root")
        .arg(dir.path().join("projects"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", &home)
        .env("LANG", "C.UTF-8")
        .env("X100_CLAUDE_LOGIN", &login)
        .env("X100_CODEX_LOGIN", dir.path().join("no-codex-login"))
        // The driver runs in a sandbox whose only way out is the daemon's proxy, which sends
        // this made-up host to the fake provider and refuses every other.
        .env("ANTHROPIC_BASE_URL", common::fake_base(&provider))
        .env("X100_PROXY_RESOLVE", common::FAKE_RESOLVE)
        .env("X100_NO_KEYCHAIN", "1")
        .env("X100_DOCKER", "/nonexistent/x100-test/docker")
        .env("X100_GH", "/nonexistent/x100-test/gh")
        .stdin(Stdio::null())
        .spawn()
        .expect("x100d starts");
    let mut daemon = Daemon(child);
    let mut options = Options::new(data.join("x100d.sock"), "test");
    options.startup_timeout = PATIENCE;
    let client = rt.block_on(Client::connect(options)).unwrap();

    // A first message the CEO sets the project up from, then writes the BRD for and reports, on
    // Claude Code's terminal.
    let (project, driver) = rt.block_on(async {
        client
            .request(SetDaemonSetting {
                setting: DaemonSetting::TurnsWithoutProgress(1),
            })
            .await
            .unwrap();
        let settings = Settings {
            ask_me_about: AskMeAbout::HighRiskOnly,
            budget: Money::from_dollars(5),
            lab_share: Percent::new(10).unwrap(),
            allowed: Default::default(),
        };
        let project = client
            .request(CreateProject {
                dir: None,
                settings: Some(settings),
            })
            .await
            .unwrap()
            .project;
        client
            .send_chat(
                project,
                "Write the BRD for a todo app, then report.",
                vec![],
                None,
            )
            .await
            .unwrap();
        let ceo = client
            .wait_for(PATIENCE, |s| {
                let p = s.projects.get(&project)?;
                let ceo = p.project()?.ceo;
                matches!(
                    p.agent(ceo)?.state,
                    AgentState::Waiting(Waiting::ForDriver(_))
                )
                .then_some(ceo)
            })
            .await
            .unwrap();
        provider.reply(Reply::Tool {
            name: "mcp__x100__set_up_project".into(),
            input: json!({
                "name": "Todo app",
                "intent": "A todo list in the browser.",
                "brief": "Write the BRD for a todo app, then report.",
            }),
        });
        provider.reply(Reply::Text("Setting it up.".into()));
        let harness = client
            .with_state(|s| {
                let terminal = HarnessKind::ClaudeCodeTerminal;
                s.daemon
                    .harnesses()
                    .find(|h| h.kind == terminal)
                    .map(|h| h.id)
            })
            .expect("Claude Code's terminal harness is registered");
        let what = HarnessModel {
            harness,
            model: ModelName("claude-haiku-5-5".into()),
        };
        let allow = SettingChange::Allow {
            what,
            allowed: true,
        };
        client.change_setting(project, allow).await.unwrap();
        // The task's id is known once the project is set up, seconds before the next driver
        // starts and asks for its replies.
        let task = client
            .wait_for(PATIENCE, |s| {
                s.projects.get(&project)?.tasks().next().map(|t| t.task.id)
            })
            .await
            .unwrap();
        for reply in [
            Reply::Tool {
                name: "mcp__x100__write_spec".into(),
                input: json!({ "spec": "brd", "status": "draft", "body": "# Todo\n" }),
            },
            Reply::Tool {
                name: "mcp__x100__report".into(),
                input: json!({ "task": task, "outcome": "completed", "summary": "Done." }),
            },
            Reply::Text("The BRD is written.".into()),
        ] {
            provider.reply(reply);
        }
        let driver = client
            .wait_for(PATIENCE, |s| {
                let p = s.projects.get(&project)?;
                let done = matches!(p.task(task)?.state, TaskState::Done(..));
                let driver = *p.agent(ceo)?.drivers.last()?;
                (done && p.driver(driver)?.state == DriverState::Warm).then_some(driver)
            })
            .await
            .unwrap();
        (project, driver)
    });

    // Opening the project's Team tab opens the CEO, who has no agents under it yet, on what it
    // is working in.
    let session_client = client.clone();
    let (team, vcx) = cx.add_window_view(move |_, cx| {
        let session = cx.new(|cx| Session::new(session_client, cx));
        TeamView::new(session, project, cx)
    });
    let vcx = vcx.clone();
    let until = Instant::now() + PATIENCE;
    let (shown, want) = loop {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        let output = vcx.read(|cx| team.read(cx).output());
        let shown = output.map(|o| vcx.read(|cx| o.read(cx).shown(cx)));
        let chunks = rt.block_on(read_all(&client, project, driver));
        let want = bare_replay(&chunks);
        // The scratch driver's screen first, then the worktree driver's.
        if let Some(shown) = shown.filter(|s| s.len() == 2 && s[1].screen == want) {
            break (shown, want);
        }
        assert!(
            Instant::now() < until,
            "the CEO's view never showed its driver's screen"
        );
    };
    assert_eq!(shown[1].terminal_of, Some(driver));
    assert!(want.iter().any(|line| !line.trim().is_empty()), "{want:#?}");
    assert!(
        vcx.read(|cx| team.read(cx).reflection().is_none()),
        "memories, skills and trials are not shown before their milestones"
    );

    let attempts = provider.proxy_attempts();
    assert!(
        attempts.iter().all(|a| a == "api.anthropic.com:443"),
        "{attempts:?}"
    );
    nix_terminate(&mut daemon);
    drop(rt);
}

/// SIGTERM, and waits for the daemon to stop its drivers and exit.
fn nix_terminate(daemon: &mut Daemon) {
    let pid = daemon.0.id().to_string();
    let status = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(status.success());
    let exited = daemon.0.wait().unwrap();
    assert!(exited.success(), "the daemon exited with {exited}");
}
