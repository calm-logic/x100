//! Terminal drivers through the real daemon, with the client library as the only way in: a new
//! project's CEO, on the real `claude` or `codex` program in a pseudo-terminal on a (dummy) local
//! login, answers "hi", sets the project up from the brief that follows, and on a new driver in
//! the project's worktree writes the BRD and reports through the daemon's tools. That driver's
//! terminal is followed live by one client and, from part way through, by a second. See
//! `common/real.rs` for how no request leaves the machine.

mod common;

use std::collections::BTreeMap;

use common::real::{PATIENCE, Program, Real, Watch, read_all};
use common::{gone, group_members, running};
use serde_json::json;
use x100_core::agent::AgentState;
use x100_core::driver::{DriverEvent, DriverState, EndReason, RetireReason, View};
use x100_core::effort::Effort;
use x100_core::event::ProjectEvent;
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::id::{AgentId, DriverId, ProjectId, TaskId};
use x100_core::message::Route;
use x100_core::spec::SpecKind;
use x100_core::task::{Outcome, TaskState};
use x100_core::terminal::ChunkKind;
use x100_drivers::terminal::Emulator;
use x100_drivers::testing::Reply;

const BRD: &str = "# A todo app\n\nAdd, tick off and remove todos in the browser.\n";

struct Ran {
    project: ProjectId,
    ceo: AgentId,
    driver: DriverId,
}

const NAME: (&str, &str) = ("Todo app", "todo-app");
const INTENT: &str = "A todo list in the browser.";

/// What the CEO is given to set the project up: the arguments of `set_up_project`.
fn set_up_arguments() -> serde_json::Value {
    json!({
        "name": NAME.0,
        "intent": INTENT,
        "brief": "Write the BRD for a todo app, then report.",
    })
}

/// "hi" to a new project, answered by the CEO on a driver in the scratch directory. Then the
/// brief: on the same driver the model calls `set_up_project` with `set_up`, the turn ends and
/// that driver retires, and the task's replies are queued once its id is known, which is seconds
/// before a new driver starts in the worktree. That driver is followed by one client from its
/// start. Once the task is done a second client subscribes, late, and a message from the builder
/// gives the driver a second turn, which the late client follows live. Checks what every terminal
/// driver must show.
async fn run_brief(r: &Real, set_up: Reply, replies: impl FnOnce(TaskId) -> Vec<Reply>) -> Ran {
    let project = r.project().await;
    r.client
        .send_chat(project, "hi", vec![], None)
        .await
        .unwrap();
    let ceo = r.ceo_waits_for_a_driver().await;
    r.provider
        .reply(Reply::Text("Hello! What would you like to build?".into()));
    r.allow(project).await;
    r.until("the CEO says hello and goes idle", |p| {
        let replied = p
            .messages()
            .any(|m| m.message.route == Route::CeoToBuilder && m.message.body.contains("Hello!"));
        (replied && p.agent(ceo)?.state == AgentState::Idle).then_some(())
    })
    .await;
    let s = r.client.project(project).unwrap();
    assert_eq!(s.tasks().count(), 0, "a greeting opens no task");
    assert_eq!(s.project().unwrap().name(), None);

    r.provider.reply(set_up);
    r.provider.reply(Reply::Text("Setting it up.".into()));
    r.client
        .send_chat(
            project,
            "Write the BRD for a todo app, then report.",
            vec![],
            None,
        )
        .await
        .unwrap();
    let task = r
        .until("the CEO sets the project up", |p| {
            p.tasks().next().map(|t| t.task.id)
        })
        .await;
    for reply in replies(task) {
        r.provider.reply(reply);
    }
    r.provider.reply(Reply::Text("Nothing else.".into()));
    let (scratch, driver) = r
        .until("the CEO's driver in the worktree starts", |p| {
            match p.agent(ceo)?.drivers.as_slice() {
                [scratch, driver] => Some((*scratch, *driver)),
                _ => None,
            }
        })
        .await;
    // Followed from its start, before anything else, so its first turn arrives live.
    let first = Watch::new(
        r.client
            .follow_terminal(project, driver, None)
            .await
            .unwrap(),
    );
    let s = r.client.project(project).unwrap();
    let project_state = s.project().unwrap();
    assert_eq!(project_state.name(), Some(NAME.0));
    let code = r.projects_root().join(NAME.1);
    assert_eq!(project_state.code_dir.as_deref(), Some(code.as_path()));
    assert_eq!(
        s.driver(scratch).unwrap().state,
        DriverState::Ended(EndReason::Retired(RetireReason::ProjectSetUp))
    );

    r.until("the brief's task is done", |p| {
        matches!(p.task(task)?.state, TaskState::Done(..)).then_some(())
    })
    .await;
    let warm_after = |turns: u32| {
        move |p: &x100_core::fold::ProjectState| {
            let d = p.driver(driver)?;
            (d.state == DriverState::Warm && d.turns == turns).then_some(())
        }
    };
    r.until("the first turn ends", warm_after(1)).await;
    let s = r.client.project(project).unwrap();
    let record = &s.driver(driver).unwrap().driver;
    assert_eq!(record.view, View::Terminal);
    assert_eq!(record.harness, r.harness());
    assert_eq!(
        s.spec(SpecKind::Brd).expect("the BRD is written").spec.body,
        BRD
    );
    let Some(TaskState::Done(_, report)) = s.task(task).map(|t| &t.state) else {
        unreachable!()
    };
    assert_eq!(report.outcome, Outcome::Completed);
    assert_eq!(report.summary, "The BRD is written.");

    // A second client, late: it gets the terminal from its first chunk, then the second turn live.
    let backlog = read_all(&r.client, project, driver).await.len();
    let late_client = r.another_client().await;
    let mut late = Watch::new(
        late_client
            .follow_terminal(project, driver, None)
            .await
            .unwrap(),
    );
    late.until(backlog).await;
    r.client
        .send_chat(project, "Anything else?", vec![], None)
        .await
        .unwrap();
    r.until("the second turn ends", warm_after(2)).await;
    assert_eq!(
        r.client
            .project(project)
            .unwrap()
            .agent(ceo)
            .unwrap()
            .drivers,
        vec![scratch, driver],
        "one driver in the worktree did all the work"
    );

    // A program may finish drawing a turn just after the turn ends.
    late.until(backlog + 1).await;
    // The whole terminal as `terminal.read` gives it, from the first chunk, which is its size.
    let read = read_all(&r.client, project, driver).await;
    assert!(matches!(read[0].1.what, ChunkKind::Resize { .. }));
    let mut first = first;
    first.until(read.len()).await;
    late.until(read.len()).await;
    for (who, w) in [("the first client", &first), ("the late client", &late)] {
        let seen = w.seen();
        let positions: Vec<u64> = seen.iter().map(|(p, _, _)| p.0).collect();
        let expected: Vec<u64> = (1..=positions.len() as u64).collect();
        assert_eq!(positions, expected, "{who} got every chunk once, in order");
        let got: Vec<_> = seen
            .iter()
            .take(read.len())
            .map(|(p, c, _)| (*p, c.clone()))
            .collect();
        assert_eq!(got, read, "{who} got what the daemon recorded");
    }
    let replayed = Emulator::replay(late.seen().iter().take(read.len()).map(|(_, c, _)| c))
        .unwrap()
        .text();
    let recorded = Emulator::replay(read.iter().map(|(_, c)| c))
        .unwrap()
        .text();
    assert_eq!(
        replayed, recorded,
        "the followed screen is the recorded one"
    );
    assert!(!recorded.trim().is_empty(), "the screen shows something");

    // Delivered while a turn ran, not at its end.
    let log = r
        .client
        .read_events_after(project, None, 10_000)
        .await
        .unwrap();
    let at = |wanted: fn(&DriverEvent) -> bool| -> Vec<_> {
        log.iter()
            .filter(|e| matches!(&e.event, ProjectEvent::Driver { driver: d, event } if *d == driver && wanted(event)))
            .map(|e| e.at)
            .collect()
    };
    let started = at(|e| matches!(e, DriverEvent::TurnStarted { .. }));
    let ended = at(|e| matches!(e, DriverEvent::TurnEnded));
    let turns: Vec<_> = started.into_iter().zip(ended).collect();
    assert_eq!(turns.len(), 2);
    let during = |w: &Watch| {
        w.seen()
            .iter()
            .any(|(p, _, got)| p.0 > 1 && turns.iter().any(|(from, to)| got >= from && got < to))
    };
    assert!(during(&first), "no chunk arrived during a turn");
    Ran {
        project,
        ceo,
        driver,
    }
}

/// The driver's processes, as the daemon wrote them down.
fn pids_of(r: &Real, driver: DriverId) -> Vec<u32> {
    let text = std::fs::read_to_string(r.data.join("drivers.json")).unwrap();
    let map: BTreeMap<String, Vec<serde_json::Value>> = serde_json::from_str(&text).unwrap();
    map.get(&driver.to_string())
        .map(|entries| {
            entries
                .iter()
                .map(|e| e["pid"].as_u64().unwrap() as u32)
                .collect()
        })
        .unwrap_or_default()
}

async fn stop_and_check_nothing_is_left(mut r: Real, pids: &[u32]) {
    r.daemon.terminate();
    gone(pids).await;
    for pid in pids {
        assert_eq!(
            group_members(*pid),
            Vec::<u32>::new(),
            "process group {pid}"
        );
    }
    r.assert_nothing_else_left_the_machine();
    assert_eq!(r.provider.unused(), 0, "every scripted reply was asked for");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_brief_runs_on_claude_codes_terminal_and_is_followed_live() {
    let Some(r) = Real::start(Program::Claude).await else {
        return;
    };
    let outside = r.dir.path().join("outside.txt");
    let ran = {
        let outside = outside.clone();
        let set_up = Reply::Tool {
            name: "mcp__x100__set_up_project".into(),
            input: set_up_arguments(),
        };
        run_brief(&r, set_up, move |task| {
            vec![
                // A command is put to the daemon, which refuses it: nothing runs.
                Reply::Tool {
                    name: "Bash".into(),
                    input: json!({
                        "command": format!("echo escaped > {}", outside.display()),
                        "description": "Write outside the worktree",
                    }),
                },
                Reply::Tool {
                    name: "mcp__x100__write_spec".into(),
                    input: json!({ "spec": "brd", "status": "draft", "body": BRD }),
                },
                Reply::Tool {
                    name: "mcp__x100__report".into(),
                    input: json!({
                        "task": task,
                        "outcome": "completed",
                        "summary": "The BRD is written.",
                    }),
                },
                Reply::Text("Done: the BRD is written and the task reported.".into()),
            ]
        })
        .await
    };
    assert!(
        !outside.exists(),
        "a command ran without the daemon's say-so"
    );
    // What the model was told of its command, in the request after it.
    let told = r.provider.requests().iter().any(|q| {
        q.body["messages"]
            .to_string()
            .contains("Commands are not available here yet")
    });
    assert!(told, "the command was put to the daemon and refused");

    let pids = pids_of(&r, ran.driver);
    assert_eq!(pids.len(), 1, "{pids:?}");
    assert!(running(pids[0]), "the warm driver is live");
    stop_and_check_nothing_is_left(r, &pids).await;
}

fn patch(name: &str) -> Reply {
    Reply::Freeform {
        name: "apply_patch".into(),
        input: format!("*** Begin Patch\n*** Add File: {name}\n+- buy milk\n*** End Patch\n"),
    }
}

/// On gpt-6-luna Codex gives the model one JavaScript `exec` tool, through which it calls MCP
/// tools as functions of `tools`.
fn exec(code: String) -> Reply {
    Reply::Freeform {
        name: "exec".into(),
        input: code,
    }
}

fn codex_set_up() -> Reply {
    let arguments = set_up_arguments();
    exec(format!(
        "const r = await tools.mcp__x100__set_up_project({arguments}); text(JSON.stringify(r));"
    ))
}

fn codex_replies(task: TaskId) -> Vec<Reply> {
    let spec = json!({ "spec": "brd", "status": "draft", "body": BRD });
    let report = json!({ "task": task, "outcome": "completed", "summary": "The BRD is written." });
    vec![
        // A file outside the worktree, which Codex's sandbox refuses, and one in it, which it lets
        // the model write without asking.
        patch("../escaped.md"),
        patch("todo.md"),
        exec(format!(
            "const r = await tools.mcp__x100__write_spec({spec}); text(JSON.stringify(r));"
        )),
        exec(format!(
            "const r = await tools.mcp__x100__report({report}); text(JSON.stringify(r));"
        )),
        Reply::Text("Done: the BRD is written and the task reported.".into()),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_brief_runs_on_codexs_terminal_and_is_followed_live() {
    let Some(r) = Real::start(Program::Codex).await else {
        return;
    };
    let ran = run_brief(&r, codex_set_up(), codex_replies).await;
    let worktree = x100_store::Git::new(r.projects_root().join(NAME.1)).worktree_path(ran.ceo);
    assert_eq!(
        std::fs::read_to_string(worktree.join("todo.md")).unwrap(),
        "- buy milk\n"
    );
    assert!(
        !worktree.join("../escaped.md").exists(),
        "a write left the worktree"
    );
    let told = r.provider.requests().iter().any(|q| {
        q.body["input"]
            .to_string()
            .contains("patch rejected: writing outside of the project")
    });
    assert!(told, "the patch outside the worktree was refused");
    let pids = pids_of(&r, ran.driver);
    assert_eq!(
        pids.len(),
        2,
        "the app-server and the terminal attached to it: {pids:?}"
    );
    stop_and_check_nothing_is_left(r, &pids).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_terminal_driver_left_by_a_killed_daemon_is_gone_after_a_restart() {
    let Some(mut r) = Real::start(Program::Codex).await else {
        return;
    };
    let ran = run_brief(&r, codex_set_up(), codex_replies).await;
    let pids = pids_of(&r, ran.driver);
    assert_eq!(pids.len(), 2, "{pids:?}");
    r.daemon.kill9();
    let outlived: Vec<u32> = pids.iter().copied().filter(|p| running(*p)).collect();
    eprintln!("left running by the killed daemon: {outlived:?} of {pids:?}");
    r.restart().await;
    gone(&pids).await;
    for pid in &pids {
        assert_eq!(
            group_members(*pid),
            Vec::<u32>::new(),
            "process group {pid}"
        );
    }
    let (project, driver) = (ran.project, ran.driver);
    r.client
        .wait_for(PATIENCE, |s| {
            let live = s.projects.get(&project)?.driver(driver)?.state.is_live();
            (!live).then_some(())
        })
        .await
        .expect("the log shows the driver ended");
    stop_and_check_nothing_is_left(r, &pids).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_harness_whose_login_is_missing_is_passed_over_and_a_new_project_runs_on_logins()
{
    let Some(mut r) = Real::start(Program::Claude).await else {
        return;
    };
    let missing = r.dir.path().join("no-login-here");
    r.restart_with("X100_CODEX_LOGIN", missing.display().to_string())
        .await;
    let daemon = r
        .client
        .wait_for(PATIENCE, |s| {
            let m = s.daemon.machines().next()?;
            (!m.passed_over.is_empty()).then(|| s.daemon.clone())
        })
        .await
        .expect("the scan says what it passed over");
    let machine = daemon.machines().next().unwrap();
    let kind_of = |id: &x100_core::id::HarnessId| daemon.harness(*id).unwrap().kind.clone();
    let mut on_machine: Vec<HarnessKind> = machine.harnesses.iter().map(kind_of).collect();
    on_machine.sort_by_key(|k| format!("{k:?}"));
    let mut expected = vec![HarnessKind::ClaudeCode, HarnessKind::ClaudeCodeTerminal];
    if installed_codex() {
        expected.push(HarnessKind::Codex);
    }
    expected.sort_by_key(|k| format!("{k:?}"));
    assert_eq!(on_machine, expected);
    assert_eq!(machine.passed_over.len(), 1);
    assert_eq!(machine.passed_over[0].kind, HarnessKind::CodexTerminal);
    assert!(
        machine.passed_over[0]
            .why
            .contains(&missing.display().to_string()),
        "{:?}",
        machine.passed_over[0]
    );
    let claude = machine
        .harnesses
        .iter()
        .find(|h| kind_of(h) == HarnessKind::ClaudeCodeTerminal)
        .unwrap();
    let login = daemon.harness(*claude).unwrap().login.clone().unwrap();
    assert!(login.0.ends_with("claude-login"), "{login:?}");

    // A project with the daemon's defaults allows the cheapest model of each terminal harness
    // and nothing headless, and its CEO works at low to medium effort.
    let project = r.client.create_project(None).await.unwrap();
    let state = r
        .client
        .wait_for(PATIENCE, |s| s.projects.get(&project)?.project().cloned())
        .await
        .unwrap();
    let allowed: Vec<_> = state.settings.allowed.iter().cloned().collect();
    assert_eq!(
        allowed,
        vec![HarnessModel {
            harness: *claude,
            model: ModelName("claude-haiku-5-5".into()),
        }]
    );
    let ceo_range = state.team.effort(state.ceo).unwrap();
    assert_eq!(
        (ceo_range.min(), ceo_range.max()),
        (Effort::Low, Effort::Medium)
    );
    r.daemon.terminate();
}

fn installed_codex() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("codex").is_file()))
}
