//! M9's check, through the daemon as a child process on the fake harness posing as two providers,
//! on the fake clock with a seed: three failures in a row report the task failed to its
//! delegator, and killing the daemon mid-task, every working agent carries on after the restart.

mod common;
#[path = "handoff/rig.rs"]
mod rig;

use rig::*;
use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::{DriverState, Failure, HandoffCause, StartReason, TakeoverCause};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::party::Party;
use x100_core::task::{Outcome, Report, TaskFailure, TaskState};

fn reported(p: &ProjectState, task: TaskId) -> Option<Report> {
    match &p.task(task)?.state {
        TaskState::InReview(_, r) | TaskState::Promoting(_, r) | TaskState::Done(_, r) => {
            Some(r.clone())
        }
        _ => None,
    }
}

fn worker(p: &ProjectState, role: &str) -> Option<(AgentId, TaskId)> {
    let agent = p.agents().find(|a| a.role == role)?.id;
    let task = p
        .tasks()
        .find(|t| t.state.assignment().is_some_and(|a| a.assignee == agent))?
        .task
        .id;
    Some((agent, task))
}

const THREE: &str = r#"
[[turn]]
when = { input_contains = "A task for you: THREE" }
steps = [
  { call = { delegate = { to = { new_worker = { role = "Crasher", charter = "Crashes." } }, task = { new = { title = "Crash", ask = "Crash every time.", attachments = [] } }, effort = "low" } }, save = "job" },
  { call = { wait = { on = { report = "{{job.task}}" }, task = "{{input:Task}}", progress = { note = "Waiting on the crasher.", done = 10 } } } },
]

[[turn]]
when = { input_contains = "A task for you: Crash" }
repeat = true
steps = [{ text = "about to crash" }, { crash = 4 }]

[[turn]]
when = { input_contains = "Report on: " }
repeat = true
steps = [
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "The crasher's report is in." } } },
]
"#;

#[tokio::test(flavor = "multi_thread")]
async fn three_failures_in_a_row_report_the_task_failed_to_its_delegator() {
    let turns = format!("{}{THREE}", set_up("THREE"));
    let rig = Rig::start("three", &turns).await;
    let brief = rig.brief("THREE").await;
    let ceo = rig.ceo;
    let (crasher, task) = rig
        .until("the crasher has its task", |p| worker(p, "Crasher"))
        .await;
    let report = rig
        .until("the crasher's task is reported", |p| reported(p, task))
        .await;
    let Outcome::Failed(TaskFailure::DriverFailures(failures)) = &report.outcome else {
        panic!("reported failed with each failure: {report:?}")
    };
    assert_eq!(failures.len(), 3);
    assert!(
        failures
            .iter()
            .all(|f| *f == Failure::ProcessExited { code: Some(4) })
    );

    // The harness that failed is avoided while the other is there; with both failed, the skip is
    // dropped rather than leaving nothing to try.
    let drivers = rig.drivers_of(crasher);
    let providers: Vec<String> = drivers.iter().map(|d| rig.provider(d.driver.id)).collect();
    assert_eq!(providers, [CHEAP, DEAR, CHEAP]);
    for pair in drivers.windows(2) {
        let StartReason::Handoff(h) = &pair[1].driver.why else {
            panic!("a takeover: {:?}", pair[1].driver.why)
        };
        assert_eq!(h.replaces, pair[0].driver.id);
        assert!(matches!(
            h.cause,
            HandoffCause::Takeover(TakeoverCause::Failed(Failure::ProcessExited { .. }))
        ));
    }

    // The report reached the CEO, who delegated it, and the CEO carried on.
    let log = rig.log().await;
    assert!(log.iter().any(|e| matches!(&e.event,
        ProjectEvent::Agent { agent, event: AgentEvent::Delivered { what: InboxItem::Report { task: t }, .. } }
            if *agent == ceo && *t == task)));
    let s = rig.state();
    assert_eq!(
        s.task(task).unwrap().state.assignment().unwrap().delegator,
        Party::Agent(ceo)
    );
    rig.until("the CEO reports its own task", |p| reported(p, brief))
        .await;
    // It stopped trying: no driver after the third.
    assert_eq!(rig.drivers_of(crasher).len(), 3);
    assert!(
        rig.state()
            .drivers()
            .all(|d| d.driver.agent != crasher || !d.state.is_live())
    );
    rig.finish().await;
}

const RESTART: &str = r#"
[[turn]]
when = { input_contains = "A task for you: RESTART" }
steps = [
  { call = { delegate = { to = { new_worker = { role = "Part A", charter = "Builds part A." } }, task = { new = { title = "Part A", ask = "Build part A.", attachments = [] } }, effort = "low" } } },
  { call = { delegate = { to = { new_worker = { role = "Part B", charter = "Builds part B." } }, task = { new = { title = "Part B", ask = "Build part B.", attachments = [] } }, effort = "low" } } },
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Both parts are delegated." } } },
]

[[turn]]
when = { start_pack_contains = "You take over", input_contains = "A task for you: Part A" }
repeat = true
steps = [
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Part A, after the restart." } } },
]

[[turn]]
when = { input_contains = "A task for you: Part A" }
steps = [
  { write = "a.txt", content = "part A, before the restart" },
  { run = "sleep 600" },
]

[[turn]]
when = { input_contains = "A task for you: Part B" }
repeat = true
steps = [
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Part B." } } },
]
"#;

#[tokio::test(flavor = "multi_thread")]
async fn killing_the_daemon_mid_task_every_working_agent_carries_on_after_the_restart() {
    let turns = format!("{}{RESTART}", set_up("RESTART"));
    let mut rig = Rig::start("restart", &turns).await;
    rig.brief("RESTART").await;
    let (a, a_task) = rig
        .until("part A is delegated", |p| worker(p, "Part A"))
        .await;
    let (b, b_task) = rig
        .until("part B is delegated", |p| worker(p, "Part B"))
        .await;
    let first = rig.begins(a, "run").await;
    let before = rig.state();
    assert_eq!(before.agent(a).unwrap().state, AgentState::Working);
    assert_eq!(
        before.agent(b).unwrap().state,
        AgentState::Working,
        "B waits its turn, working"
    );
    let pids: Vec<u32> = rig
        .env
        .recorded_pids()
        .into_iter()
        .map(|(_, p)| p)
        .collect();

    rig.kill_and_restart_daemon().await;
    common::gone(&pids).await;

    let a_report = rig
        .until("part A is reported", |p| reported(p, a_task))
        .await;
    let b_report = rig
        .until("part B is reported", |p| reported(p, b_task))
        .await;
    assert_eq!(a_report.summary, "Part A, after the restart.");
    assert_eq!(b_report.summary, "Part B.");

    let drivers = rig.drivers_since(a, first);
    assert_eq!(drivers.len(), 2, "{drivers:#?}");
    assert_eq!(
        drivers[0].state,
        DriverState::Failed(Failure::DaemonRestarted)
    );
    let StartReason::Handoff(h) = &drivers[1].driver.why else {
        panic!("A's work is taken over: {:?}", drivers[1].driver.why)
    };
    assert_eq!(h.replaces, first);
    assert_eq!(
        h.cause,
        HandoffCause::Takeover(TakeoverCause::Failed(Failure::DaemonRestarted))
    );
    assert_eq!(
        std::fs::read_to_string(rig.worktree(a).join("a.txt")).unwrap(),
        "part A, before the restart"
    );
    let pack = rig.start_pack(drivers[1].driver.id).await;
    assert!(
        pack.contains("the daemon restarted while it was working"),
        "{pack}"
    );
    rig.finish().await;
}
