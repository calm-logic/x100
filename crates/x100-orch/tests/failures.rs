//! Every delegated task ends in a report to its delegator: when the assignee's driver crashes and
//! the next one finishes, when its drivers keep crashing, and when it never reports or waits.

mod common;

use common::*;
use x100_core::agent::AgentEvent;
use x100_core::driver::{DriverState, Failure};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::party::Party;
use x100_core::task::{Outcome, Report, TaskFailure, TaskState};

const SCRIPT: &str = r#"
[harness]
name = "fake-failures"

[[model]]
model = "fake-long"
context_window = 200000
cache_lifetime_secs = 604800
efforts = ["low", "medium", "high", "max"]
price = { input = 1.0, output = 5.0 }

# The CEO hands each kind of trouble to a new worker and waits on its report.
[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Trouble: " }
steps = [
  { call = { delegate = { to = { new_worker = { role = "Worker", charter = "Does what it is given." } }, task = { new = { title = "{{input:Trouble}}", ask = "As the brief says.", attachments = [] } }, effort = "low" } }, save = "job" },
  { call = { wait = { on = { report = "{{job.task}}" }, task = "{{input:Current task}}", progress = { note = "Delegated; waiting on the worker.", done = 10 } } } },
]

[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Report on: " }
steps = [
  { call = { review = { task = "{{input:Report on}}", verdict = "accept" } } },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "The worker's report is in." } } },
]

# A worker's second driver, after the first crashed.
[[turn]]
when = { start_pack_contains = "which failed: its process exited with code 3" }
steps = [
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Done on the second driver." } } },
]

[[turn]]
when = { input_contains = "crash-once" }
steps = [{ text = "Working." }, { crash = 3 }]

[[turn]]
when = { input_contains = "crash-always" }
repeat = true
steps = [{ crash = 4 }]

[[turn]]
when = { input_contains = "never-finish" }
repeat = true
steps = [{ text = "Still thinking." }]

[[turn]]
when = { input_contains = "neither reported nor waited on" }
repeat = true
steps = [{ text = "Still thinking." }]
"#;

/// The brief names the trouble in one word; the CEO passes it on as the worker's task title.
async fn delegate(trouble: &str) -> (World, TaskId) {
    let w = World::new(Setup::new(SCRIPT)).await;
    let brief = w.send(&format!("Trouble: {trouble}")).await;
    (w, brief)
}

fn worker_task(s: &ProjectState) -> (AgentId, TaskId) {
    let worker = agent(s, "Worker");
    let task = s
        .tasks()
        .find(|t| t.state.assignment().is_some_and(|a| a.assignee == worker))
        .expect("the worker's task")
        .task
        .id;
    (worker, task)
}

fn report(s: &ProjectState, task: TaskId) -> Report {
    match &s.task(task).unwrap().state {
        TaskState::InReview(_, r) | TaskState::Promoting(_, r) | TaskState::Done(_, r) => r.clone(),
        other => panic!("not reported: {}", other.name()),
    }
}

fn reached_delegator(w: &World, task: TaskId) -> bool {
    let s = w.state();
    let Some(Party::Agent(delegator)) = s
        .task(task)
        .unwrap()
        .state
        .assignment()
        .map(|a| a.delegator)
    else {
        return false;
    };
    w.log().iter().any(|e| matches!(&e.event,
        ProjectEvent::Agent { agent, event: AgentEvent::Delivered { what: InboxItem::Report { task: t }, .. } }
            if *agent == delegator && *t == task))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_whose_driver_crashes_is_finished_by_the_next_and_reported() {
    let (w, brief) = delegate("crash-once").await;
    w.settle().await;
    let s = w.state();
    let (worker, task) = worker_task(&s);
    let drivers = &s.agent(worker).unwrap().drivers;
    assert_eq!(drivers.len(), 2);
    assert_eq!(
        s.driver(drivers[0]).unwrap().state,
        DriverState::Failed(Failure::ProcessExited { code: Some(3) })
    );
    let r = report(&s, task);
    assert_eq!(
        (r.outcome, r.summary.as_str()),
        (Outcome::Completed, "Done on the second driver.")
    );
    assert!(reached_delegator(&w, task));
    assert!(matches!(s.task(brief).unwrap().state, TaskState::Done(..)));
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_whose_drivers_keep_failing_is_reported_failed_after_three() {
    let (w, brief) = delegate("crash-always").await;
    w.until("the CEO has the failure report and finishes", |s| {
        matches!(s.task(brief).map(|t| &t.state), Some(TaskState::Done(..)))
    })
    .await;
    let s = w.state();
    let (worker, task) = worker_task(&s);
    let drivers = &s.agent(worker).unwrap().drivers;
    assert_eq!(
        drivers.len(),
        3,
        "it stops trying after three failures in a row"
    );
    for d in drivers {
        assert_eq!(
            s.driver(*d).unwrap().state,
            DriverState::Failed(Failure::ProcessExited { code: Some(4) })
        );
    }
    let Outcome::Failed(TaskFailure::DriverFailures(failures)) = report(&s, task).outcome else {
        panic!("reported failed by the daemon");
    };
    assert_eq!(failures.len(), 3);
    assert!(reached_delegator(&w, task));
    // The item that kept failing is kept for whatever next wakes the worker.
    assert_eq!(s.agent(worker).unwrap().inbox.waiting().count(), 1);
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_never_reported_nor_waited_on_is_nudged_then_reported_failed() {
    let (w, brief) = delegate("never-finish").await;
    w.settle().await;
    let s = w.state();
    let (worker, task) = worker_task(&s);
    let nudges: Vec<u32> = w
        .log()
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::Agent {
                agent,
                event:
                    AgentEvent::Delivered {
                        what: InboxItem::Nudge { task: t, turns },
                        ..
                    },
            } if *agent == worker && *t == task => Some(*turns),
            _ => None,
        })
        .collect();
    assert_eq!(
        nudges,
        [1, 2],
        "two nudges, then the third turn gives it up"
    );
    assert_eq!(
        report(&s, task).outcome,
        Outcome::Failed(TaskFailure::NoProgress { turns: 3 })
    );
    let turns: u32 = s
        .agent(worker)
        .unwrap()
        .drivers
        .iter()
        .map(|d| s.driver(*d).unwrap().turns)
        .sum();
    assert_eq!(turns, 3);
    assert!(reached_delegator(&w, task));
    assert!(matches!(s.task(brief).unwrap().state, TaskState::Done(..)));
    w.finish().await;
}
