//! The owner's goal end to end: the desktop app, with no display, against a real `x100d` child
//! process on the real clock with a new project's default settings. With no project, it says "hi"
//! from the box the window opens on, which creates the project, then sends a small brief, which
//! the CEO sets the project up from. It waits for the CEO's final report and checks what a
//! builder would.
//!
//! On the builder's own logins, the work runs on the cheapest terminal models of their local
//! Claude Code and Codex. That spends their subscription, so it is ignored by default and runs
//! only when asked:
//!
//! ```sh
//! X100_REAL_LOGINS=1 cargo test -p x100-app --test real_e2e -- --ignored --nocapture
//! ```
//!
//! With `X100_E2E_OUT=DIR` the project (its code directory under `DIR/projects`, and its store with
//! the log, transcripts and recorded terminals as `DIR/store`), the daemon's output and each
//! agent's transcript and terminal screens are kept in `DIR` to be looked at.
//! They can hold the builder's account id, so `DIR` belongs outside the repository. Without it
//! everything is removed. The daemon's data directory is never under /tmp, where Codex will not
//! set up its sandbox.
//!
//! The same run also goes through every step at no cost, on the real `claude` program with the
//! model played by the fake provider on a dummy login, as `real_terminal.rs` does.

mod common;
mod e2e;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{bounds, click, pump, quiet};
use e2e::{Account, browser_check, git, keep_records, nothing_left, read_terminal};
use gpui::{Entity, TestAppContext, VisualTestContext, WindowHandle};
use serde_json::json;
use x100_app::budget::BudgetTab;
use x100_app::log::LogView;
use x100_app::shell::Shell;
use x100_app::tabs::{MainPanel, Tab};
use x100_app::team::{Shown, TeamView};
use x100_client::{Client, Options};
use x100_core::agent::AgentState;
use x100_core::driver::{DriverState, EndReason, RetireReason};
use x100_core::id::{AgentId, ProjectId, TaskId};
use x100_core::message::Route;
use x100_core::spec::SpecKind;
use x100_core::task::{Outcome, TaskState};
use x100_drivers::testing::{FakeProvider, Reply};

const BRIEF: &str = "Create a todo app as a proof of concept: one index.html with inline CSS and \
                     JavaScript, no build step and no dependencies. I can add a todo, tick it \
                     done, and delete it, and the list survives a reload.";

struct Daemon(Option<Child>);

impl Daemon {
    /// SIGTERM, so the daemon stops its drivers, then waits for it.
    fn stop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        let _ = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let until = Instant::now() + Duration::from_secs(60);
        while Instant::now() < until {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Where the models come from.
enum Models {
    /// The builder's own logins, through the builder's own environment.
    Real,
    /// The fake provider on localhost, with a dummy Claude login and no Codex one, so the default
    /// settings allow Claude Code's terminal alone.
    Fake {
        provider: FakeProvider,
        home: PathBuf,
        login: PathBuf,
    },
}

impl Models {
    /// A generous bound on the whole run; a cheap model on a small brief takes minutes.
    fn deadline(&self) -> Duration {
        match self {
            Models::Real => Duration::from_secs(45 * 60),
            Models::Fake { .. } => Duration::from_secs(180),
        }
    }

    fn environment(&self, command: &mut Command) {
        match self {
            // As if the builder started the daemon, less anything that would point it elsewhere.
            Models::Real => {
                for var in [
                    "X100_DATA_DIR",
                    "X100_FAKE_SCRIPTS",
                    "X100_FAKE_HARNESS",
                    "X100_CLAUDE_LOGIN",
                    "X100_CODEX_LOGIN",
                    "ANTHROPIC_BASE_URL",
                    "OPENAI_BASE_URL",
                ] {
                    command.env_remove(var);
                }
            }
            Models::Fake {
                provider,
                home,
                login,
            } => {
                command
                    .env_clear()
                    .env("PATH", std::env::var("PATH").unwrap())
                    .env("HOME", home)
                    .env("LANG", "C.UTF-8")
                    .env("X100_CLAUDE_LOGIN", login)
                    .env("X100_CODEX_LOGIN", home.join("no-codex-login"))
                    // The drivers' only way out is the daemon's proxy, which sends this made-up
                    // host to the fake provider and refuses every other.
                    .env("ANTHROPIC_BASE_URL", common::fake_base(provider))
                    .env("X100_PROXY_RESOLVE", common::FAKE_RESOLVE)
                    .env("X100_NO_KEYCHAIN", "1")
                    // No Homebox or gh of the builder's is looked for.
                    .env("X100_DOCKER", "/nonexistent/x100-test/docker")
                    .env("X100_GH", "/nonexistent/x100-test/gh");
            }
        }
    }

    /// What the fake model says to "hi" in a project that is not set up: hello, sent with
    /// `message` as the guide tells it, since the text of its turn does not reach the builder.
    fn greet(&self) {
        if let Models::Fake { provider, .. } = self {
            provider.reply(Reply::Tool {
                name: "mcp__x100__message".into(),
                input: json!({
                    "to": "builder",
                    "body": "Hello! What would you like to build?",
                    "attachments": [],
                }),
            });
            provider.reply(Reply::Text("Said hello.".into()));
        }
    }

    /// What the fake model does with the brief before the project is set up: sets it up, with
    /// the brief as it understood it, and ends the turn.
    fn set_up(&self) {
        let Models::Fake { provider, .. } = self else {
            return;
        };
        provider.reply(Reply::Tool {
            name: "mcp__x100__set_up_project".into(),
            input: json!({ "name": "Todo app", "intent": "A todo list in one page.", "brief": BRIEF }),
        });
        provider.reply(Reply::Text("A todo app: setting it up now.".into()));
    }

    /// What the fake model does with the brief, as a model told what the start pack says would:
    /// answer first, write the BRD, build the page, keep its note, report, and reply.
    fn script(&self, task: TaskId, worktree: &Path) {
        let Models::Fake { provider, .. } = self else {
            return;
        };
        let tool = |name: &str, input| Reply::Tool {
            name: format!("mcp__x100__{name}"),
            input,
        };
        for reply in [
            tool(
                "message",
                json!({ "to": "builder", "body": "A one-page todo app. Building it now.", "attachments": [] }),
            ),
            tool(
                "write_spec",
                json!({ "spec": "brd", "status": "draft", "body": "# Todo\n\nAdd, tick and delete todos; they survive a reload.\n" }),
            ),
            Reply::Tool {
                name: "Write".into(),
                input: json!({ "file_path": worktree.join("index.html"), "content": e2e::TODO_PAGE }),
            },
            tool(
                "update_task",
                json!({ "task": task, "progress": { "note": "index.html written.", "done": 90 } }),
            ),
            tool(
                "report",
                json!({ "task": task, "outcome": "completed", "summary": "index.html is a todo app." }),
            ),
            Reply::Text("Done: index.html is on main. Open it in a browser.".into()),
        ] {
            provider.reply(reply);
        }
    }
}

/// What the run saw while it went, for the account and the checks.
#[derive(Default)]
struct Seen {
    /// The CEO's header in the Team tab while it worked: its driver's pill.
    ceo_working_on: Option<String>,
    /// The CEO's live terminal screen was drawn in its view.
    ceo_terminal_drawn: bool,
}

#[test]
#[ignore = "runs a real project on the builder's own logins; see the module docs"]
fn a_todo_app_is_built_from_the_brief_on_the_builders_logins() {
    if std::env::var("X100_REAL_LOGINS").as_deref() != Ok("1") {
        eprintln!("SKIPPED: set X100_REAL_LOGINS=1 to run on the builder's own logins");
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    build_from_the_brief(&rt, Models::Real);
}

#[test]
fn the_same_run_goes_through_on_the_fake_provider() {
    if !e2e::installed("claude") {
        eprintln!("SKIPPED: claude is not installed on this machine");
        return;
    }
    // The fake provider is reached through the sandbox's proxy, which is the point of the run.
    if common::available::skip_without_sandbox() {
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (home, login) = (dir.path().join("home"), dir.path().join("claude-login"));
    std::fs::create_dir_all(&home).unwrap();
    e2e::dummy_login(&login);
    let provider = rt.block_on(FakeProvider::start()).unwrap();
    build_from_the_brief(
        &rt,
        Models::Fake {
            provider,
            home,
            login,
        },
    );
}

fn build_from_the_brief(rt: &tokio::runtime::Runtime, models: Models) {
    let started = Instant::now();
    let scratch = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = scratch.path().join("data");
    let keep = match (&models, std::env::var_os("X100_E2E_OUT")) {
        (Models::Real, Some(out)) => PathBuf::from(out),
        _ => scratch.path().join("out"),
    };
    std::fs::create_dir_all(&keep).unwrap();
    let projects = keep.join("projects");
    let daemon_log = keep.join("daemon.log");

    let log = std::fs::File::create(&daemon_log).unwrap();
    let mut command = Command::new(common::binary("x100d"));
    command
        .arg("--data-dir")
        .arg(&data)
        // Never the builder's own `~/x100`, on their logins too.
        .arg("--projects-root")
        .arg(&projects)
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    models.environment(&mut command);
    let mut daemon = Daemon(Some(command.spawn().expect("x100d starts")));

    let mut options = Options::new(data.join("x100d.sock"), "e2e");
    options.startup_timeout = Duration::from_secs(60);
    let client = rt
        .block_on(Client::connect(options))
        .expect("the daemon answers");
    let cx = TestAppContext::single();
    cx.update(x100_app::init);
    let window = cx
        .update(|cx| x100_app::open_window(cx, client.clone()))
        .expect("a window");
    let mut vcx = VisualTestContext::from_window(window.into(), &cx);

    // No project: the window opens on the box for the first message. "hi" creates the project,
    // and the CEO only answers, since it does not know what the project is yet.
    common::pump_until(&mut vcx, "the message box is drawn", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    models.greet();
    say(&mut vcx, "hi");
    // Waited for by pumping: the app creates the project and sends in steps the window runs.
    let project = common::pump_for(
        &mut vcx,
        &client,
        ("the CEO answers hi", Duration::from_secs(600)),
        |s| {
            s.projects.iter().find_map(|(id, p)| {
                let answered = p.messages().any(|m| m.message.route == Route::CeoToBuilder);
                let idle = p.agent(p.project()?.ceo)?.state == AgentState::Idle;
                (answered && idle).then_some(*id)
            })
        },
    );
    let p = client.project(project).expect("the project");
    assert!(!p.project().unwrap().is_set_up(), "set up from a greeting");
    assert_eq!(p.tasks().count(), 0, "a greeting opened a task");

    // The brief: the CEO sets the project up, which opens its task, and its work goes on in the
    // project's worktree on a new driver.
    models.set_up();
    say(&mut vcx, BRIEF);
    let (task, ceo, project_dir) = common::pump_for(
        &mut vcx,
        &client,
        ("the set-up", Duration::from_secs(600)),
        |s| {
            let p = s.projects.get(&project)?;
            let task = p.tasks().next()?.task.id;
            let pr = p.project()?;
            Some((task, pr.ceo, pr.code_dir.clone()?))
        },
    );
    models.script(task, &worktree(&project_dir, ceo));
    common::pump_until(&mut vcx, "the main panel opens", |vcx| {
        bounds(vcx, "tab-Team").is_some()
    });

    let seen = watch(
        &mut vcx,
        window,
        &client,
        (project, task),
        &models,
        &daemon_log,
    );
    let p = client.project(project).expect("the project");
    let account = Account::read(rt, &client, project, &p, started.elapsed());
    eprintln!("{account}");
    keep_records(rt, &client, project, &p, &keep);

    // What a builder would check.
    let replies: Vec<_> = p
        .messages()
        .filter(|m| m.message.route == Route::CeoToBuilder)
        .collect();
    assert!(!replies.is_empty(), "the CEO never replied in the chat");
    let brd = p.spec(SpecKind::Brd).expect("a BRD is written");
    assert!(!brd.spec.body.trim().is_empty());
    let Some(TaskState::Done(_, report)) = p.task(task).map(|t| &t.state) else {
        panic!("the brief's task is not done: {:?}", p.task(task));
    };
    assert_eq!(report.outcome, Outcome::Completed);
    let html = git(&project_dir, &["show", "main:index.html"]).expect("index.html is on main");
    let lower = html.to_lowercase();
    for part in ["<input", "localstorage"] {
        assert!(lower.contains(part), "index.html has no {part}");
    }
    assert!(
        lower.contains("<ul") || lower.contains("<ol") || lower.contains("<li"),
        "index.html has no list"
    );
    let seen_on = seen
        .ceo_working_on
        .expect("the Team tab never showed the CEO working on a driver");
    assert!(seen_on.contains("terminal"), "{seen_on}");
    assert!(
        seen.ceo_terminal_drawn,
        "the CEO's terminal was never drawn"
    );
    for d in &p.agent(ceo).unwrap().drivers {
        assert!(
            !read_terminal(rt, &client, project, *d).is_empty(),
            "driver {d} recorded no terminal"
        );
    }
    let budget = open_tab::<BudgetTab>(&mut vcx, window, project, Tab::Budget);
    common::pump_until(&mut vcx, "the Budget tab shows spend", |vcx| {
        vcx.read(|cx| budget.read(cx).shown().cloned())
            .is_some_and(|m| m.view.project.spent.micros() > 0)
    });
    let log_view = open_tab::<LogView>(&mut vcx, window, project, Tab::Log);
    log_has(&mut vcx, &log_view, &["Chose ", "Reported on "]);
    match browser_check(&html, scratch.path()) {
        Ok(said) => eprintln!("In headless Chromium: {said}"),
        Err(e2e::Browser::Missing(why)) => eprintln!("Not checked in a browser: {why}"),
        Err(e2e::Browser::Failed(why)) => panic!("in headless Chromium: {why}"),
    }

    let drivers = &p.agent(ceo).unwrap().drivers;
    assert!(drivers.len() >= 2, "the work went on on a new driver");
    assert_eq!(
        p.driver(drivers[0]).map(|d| d.state.clone()),
        Some(DriverState::Ended(EndReason::Retired(
            RetireReason::ProjectSetUp
        ))),
        "the driver that set the project up retired"
    );
    assert!(project_dir.starts_with(&projects));

    drop(vcx);
    daemon.stop();
    // The project's store, kept with its code for whoever looks at the run.
    let store = data.join("projects").join(project.to_string());
    let _ = Command::new("cp")
        .arg("-a")
        .arg(&store)
        .arg(keep.join("store"))
        .status();
    nothing_left(&[&data, &project_dir]);
    eprintln!("The whole run took {:.0?}.", started.elapsed());
}

fn say(vcx: &mut VisualTestContext, text: &str) {
    click(vcx, "composer-input");
    vcx.simulate_input(text);
    vcx.simulate_keystrokes("enter");
    pump(vcx);
}

fn worktree(project_dir: &Path, agent: AgentId) -> PathBuf {
    project_dir
        .join(".x100")
        .join("worktrees")
        .join(agent.to_string())
}

/// Pumps the window until the brief's task is done and nothing more is running, keeping what the
/// Team tab shows of the CEO. Stops the run on a sign that the logins should not be used further.
fn watch(
    vcx: &mut VisualTestContext,
    window: WindowHandle<Shell>,
    client: &Client,
    (project, task): (ProjectId, TaskId),
    models: &Models,
    daemon_log: &Path,
) -> Seen {
    let main = main_panel(vcx, window, project);
    let team = vcx.read(|cx| main.read(cx).team().clone());
    let until = Instant::now() + models.deadline();
    let mut seen = Seen::default();
    let mut said = Instant::now();
    let mut settled_since: Option<Instant> = None;
    loop {
        pump(vcx);
        observe(vcx, &team, &mut seen);
        let log = std::fs::read_to_string(daemon_log).unwrap_or_default();
        for sign in [
            "usage window is",
            "asks for a login",
            "waiting on its terminal",
        ] {
            assert!(
                !log.contains(sign),
                "stopping: the daemon's output says \"{sign}\"; see {}",
                daemon_log.display()
            );
        }
        let p = client.project(project).expect("the project");
        let state = p.task(task).map(|t| t.state.clone());
        if let Some(TaskState::InReview(_, r)) = &state
            && matches!(r.outcome, Outcome::Failed(_))
        {
            panic!("the CEO reported the brief failed: {}", r.summary);
        }
        let done = matches!(state, Some(TaskState::Done(..))) && quiet(&p);
        settled_since = match (done, settled_since) {
            (true, None) => Some(Instant::now()),
            (true, s) => s,
            (false, _) => None,
        };
        if settled_since.is_some_and(|s| s.elapsed() > Duration::from_secs(3)) {
            return seen;
        }
        if said.elapsed() > Duration::from_secs(30) {
            said = Instant::now();
            let turns = p.drivers().map(|d| d.turns).sum::<u32>();
            let spent = p.budget().map(|b| b.project.spent.micros()).unwrap_or(0);
            eprintln!(
                "... task {:?}, {} agents, {turns} turns ended, {spent} micro-dollars",
                state.as_ref().map(TaskState::name),
                p.agents().count()
            );
        }
        assert!(Instant::now() < until, "the run passed its deadline");
    }
}

/// Keeps the CEO's pill while it works, and opens its output when it runs agents, since that is
/// where its terminal is.
fn observe(vcx: &mut VisualTestContext, team: &Entity<TeamView>, seen: &mut Seen) {
    let header = vcx.read(|cx| team.read(cx).header());
    if let Some(label) = header.and_then(|h| vcx.read(|cx| h.read(cx).shown()?.driver.clone()))
        && seen.ceo_working_on.is_none()
    {
        seen.ceo_working_on = Some(label);
    }
    if vcx.read(|cx| team.read(cx).output()).is_none() && bounds(vcx, "shown-Output").is_some() {
        team.update(vcx, |t, cx| t.show(Shown::Output, cx));
    }
    if let Some(output) = vcx.read(|cx| team.read(cx).output()) {
        let drawn = vcx
            .read(|cx| output.read(cx).shown(cx))
            .iter()
            .any(|s| s.terminal_of.is_some() && s.screen.iter().any(|l| !l.trim().is_empty()));
        seen.ceo_terminal_drawn |= drawn;
    }
}

fn main_panel(
    vcx: &VisualTestContext,
    window: WindowHandle<Shell>,
    project: ProjectId,
) -> Entity<MainPanel> {
    vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .main
            .clone()
    })
}

fn open_tab<V: 'static>(
    vcx: &mut VisualTestContext,
    window: WindowHandle<Shell>,
    project: ProjectId,
    tab: Tab,
) -> Entity<V> {
    let selector: &'static str = Box::leak(format!("tab-{}", tab.name()).into_boxed_str());
    click(vcx, selector);
    let main = main_panel(vcx, window, project);
    vcx.read(|cx| main.read(cx).tab_view::<V>(tab))
        .expect("the tab's view")
}

/// Pumps until the Log tab holds a line starting with each of `wanted`, scrolling to older pages.
fn log_has(vcx: &mut VisualTestContext, log: &Entity<LogView>, wanted: &[&str]) {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        pump(vcx);
        let lines: Vec<String> = vcx.read(|cx| {
            log.read(cx)
                .lines()
                .iter()
                .map(|l| l.text.clone())
                .collect()
        });
        if wanted
            .iter()
            .all(|w| lines.iter().any(|l| l.starts_with(w)))
        {
            return;
        }
        vcx.simulate_event(gpui::ScrollEvent {
            position: gpui::point(gpui::px(900.), gpui::px(500.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-3000.))),
            ..Default::default()
        });
        assert!(
            Instant::now() < until,
            "the Log never showed {wanted:?}; it has {lines:#?}"
        );
    }
}
