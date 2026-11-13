//! M9's kill test on real harnesses and real models: Claude Code and Codex, each run in a terminal
//! on the builder's own login, as the two providers, in both directions. The first driver of the
//! CEO's task is killed mid-task, and a driver of the other program takes the task over and
//! finishes it.
//!
//! It spends the builder's subscriptions, so it is ignored by default and runs only when asked:
//!
//! ```sh
//! X100_REAL_LOGINS=1 cargo test -p x100d --test handoff_real -- --ignored --nocapture
//! ```
//!
//! The daemon runs on the real clock, in the builder's own environment less anything that would
//! point it elsewhere, with a data directory and code directories of its own that are removed
//! afterwards. The project allows only the first program's cheapest model until that driver is
//! working on the task, then the other's too, so the first driver is the first program's and the
//! takeover, which avoids the harness that failed, goes to the other.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use common::real::Program;
use common::{DaemonProc, uncontained_if_no_sandbox};
use x100_client::{Client, Options};
use x100_core::action::AskMeAbout;
use x100_core::driver::{DriverState, Failure, HandoffCause, StartReason, TakeoverCause};
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessModel, ModelName};
use x100_core::id::{DriverId, ProjectId};
use x100_core::inbox::InboxItem;
use x100_core::money::{Money, Percent};
use x100_core::project::{SettingChange, Settings};
use x100_core::task::{Outcome, TaskState};
use x100_core::transcript::Item;
use x100_proto::{CreateProject, ReadTranscript};

/// Real model turns take minutes; this bounds a hang.
const PATIENCE: Duration = Duration::from_secs(30 * 60);

const BRIEF: &str = "Set this project up as \"Handoff\", for checking that work survives a crash. \
                     The work: write notes/plan.md with three short steps, then write hello.txt \
                     containing the word hello, then report the task done.";

/// Variables that would point the daemon at fakes or at other logins than the builder's own.
const NOT_PASSED: [&str; 7] = [
    "X100_DATA_DIR",
    "X100_FAKE_SCRIPTS",
    "X100_FAKE_HARNESS",
    "X100_CLAUDE_LOGIN",
    "X100_CODEX_LOGIN",
    "ANTHROPIC_BASE_URL",
    "OPENAI_BASE_URL",
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real model turns on the builder's own logins; see the module docs"]
async fn claude_code_killed_mid_task_is_taken_over_by_codex() {
    kill_test(Program::Claude, Program::Codex).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real model turns on the builder's own logins; see the module docs"]
async fn codex_killed_mid_task_is_taken_over_by_claude_code() {
    kill_test(Program::Codex, Program::Claude).await;
}

async fn kill_test(first: Program, second: Program) {
    if std::env::var("X100_REAL_LOGINS").as_deref() != Ok("1") {
        eprintln!("SKIPPED: set X100_REAL_LOGINS=1 to run on the builder's own logins");
        return;
    }
    // Not under /tmp, where Codex will not set up its sandbox's helpers.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = dir.path().join("data");
    let args = vec![
        "--data-dir".into(),
        data.clone().into_os_string(),
        "--projects-root".into(),
        dir.path().join("projects").into_os_string(),
    ];
    let env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !NOT_PASSED.contains(&k.as_str()))
        .collect();
    let mut daemon = DaemonProc::spawn_in(args, &env);
    let mut options = Options::new(data.join("x100d.sock"), "handoff-real");
    options.startup_timeout = Duration::from_secs(60);
    let client = Client::connect(options).await.expect("the daemon answers");
    uncontained_if_no_sandbox(&client).await;

    let model = |program: Program| HarnessModel {
        harness: client
            .with_state(|s| {
                s.daemon
                    .harnesses()
                    .find(|h| h.kind == program.kind())
                    .map(|h| h.id)
            })
            .unwrap_or_else(|| panic!("{program:?} is installed and logged in")),
        model: ModelName(program.model().into()),
    };
    let folder = dir.path().join("handoff");
    std::fs::create_dir_all(&folder).unwrap();
    let settings = Settings {
        ask_me_about: AskMeAbout::HighRiskOnly,
        budget: Money::from_dollars(5),
        lab_share: Percent::new(10).unwrap(),
        trial_before_commit: false,
        allowed: BTreeSet::from([model(first)]),
    };
    let project = client
        .request(CreateProject {
            dir: Some(folder.display().to_string()),
            settings: Some(settings),
        })
        .await
        .expect("the project is created")
        .project;
    client
        .wait_for(PATIENCE, |s| {
            s.projects.get(&project)?.project().map(|_| ())
        })
        .await
        .unwrap();
    client
        .send_chat(project, BRIEF, vec![], None)
        .await
        .unwrap();

    // The first program's driver working on the task, once it has done something.
    let task_driver = client
        .wait_for(PATIENCE, |s| working_on_the_task(s.projects.get(&project)?))
        .await
        .expect("the CEO's task is being worked on");
    acted(&client, project, task_driver).await;
    client
        .change_setting(
            project,
            SettingChange::Allow {
                what: model(second),
                allowed: true,
            },
        )
        .await
        .unwrap();
    kill(&data, task_driver);

    let p = client
        .wait_for(PATIENCE, |s| {
            let p = s.projects.get(&project)?;
            let t = p.tasks().next()?;
            matches!(t.state, TaskState::InReview(..) | TaskState::Done(..)).then(|| p.clone())
        })
        .await
        .expect("the task is reported");
    let task = p.tasks().next().unwrap();
    let report = match &task.state {
        TaskState::InReview(_, r) | TaskState::Done(_, r) => r,
        _ => unreachable!(),
    };
    assert_eq!(report.outcome, Outcome::Completed, "{report:?}");
    let ceo = p.project().unwrap().ceo;
    let drivers: Vec<_> = p.agent(ceo).unwrap().drivers.clone();
    let at = drivers.iter().position(|d| *d == task_driver).unwrap();
    let next = p.driver(drivers[at + 1]).expect("a driver took over");
    assert_eq!(
        p.driver(task_driver).unwrap().driver.harness,
        model(first).harness
    );
    assert_eq!(
        next.driver.harness,
        model(second).harness,
        "the other program"
    );
    let StartReason::Handoff(h) = &next.driver.why else {
        panic!("a takeover: {:?}", next.driver.why)
    };
    assert_eq!(h.replaces, task_driver);
    assert!(matches!(
        h.cause,
        HandoffCause::Takeover(TakeoverCause::Failed(
            Failure::ProcessExited { .. } | Failure::StreamBroke
        ))
    ));
    let worktree = folder.join(".x100/worktrees").join(ceo.to_string());
    let hello = std::fs::read_to_string(worktree.join("hello.txt")).unwrap_or_default();
    assert!(hello.to_lowercase().contains("hello"), "{hello:?}");
    daemon.terminate();
}

/// The CEO's driver running the turn its task's item was given to.
fn working_on_the_task(p: &ProjectState) -> Option<DriverId> {
    let ceo = p.project()?.ceo;
    let task = p.tasks().next()?.task.id;
    let a = p.agent(ceo)?;
    let (item, _) = a
        .items
        .iter()
        .find(|(_, i)| **i == InboxItem::Task { task })?;
    // Given to the turn that is running: not waiting, not yet handled.
    let given = a.inbox.unhandled().any(|u| u == *item) && !a.inbox.waiting().any(|w| w == *item);
    let d = p.acting_driver(ceo)?;
    (given && p.driver(d)?.state == DriverState::Running).then_some(d)
}

/// Waits until `driver` has made a tool call, so it is killed with work behind it.
async fn acted(client: &Client, project: ProjectId, driver: DriverId) {
    let ceo = client
        .project(project)
        .and_then(|p| p.project().map(|p| p.ceo))
        .unwrap();
    let until = std::time::Instant::now() + PATIENCE;
    loop {
        let page = client
            .request(ReadTranscript {
                project,
                agent: ceo,
                after: None,
                limit: 10_000,
            })
            .await
            .unwrap();
        let acted = page
            .entries
            .iter()
            .any(|e| e.entry.driver == driver && matches!(e.entry.item, Item::ToolCall { .. }));
        if acted {
            return;
        }
        assert!(std::time::Instant::now() < until, "{driver} never acted");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// SIGKILL to every process group of the driver's that the daemon wrote down.
fn kill(data: &std::path::Path, driver: DriverId) {
    let text = std::fs::read_to_string(data.join("drivers.json")).expect("the drivers on record");
    let map: serde_json::Value = serde_json::from_str(&text).unwrap();
    let entries = map[driver.to_string()]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!entries.is_empty(), "{driver} has a process on record");
    for e in entries {
        let pid = nix::unistd::Pid::from_raw(e["pid"].as_u64().unwrap() as i32);
        let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
    }
}
