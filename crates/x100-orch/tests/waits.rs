//! Waiting, on the fake clock: a wait past its deadline, a long wait across a stopped driver, and
//! an item whose turn never completed.

mod common;

use common::*;
use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::{DriverEvent, DriverState, EndReason, StopReason};
use x100_core::event::ProjectEvent;
use x100_core::inbox::InboxItem;
use x100_core::job::{JobState, ProjectJob, ProjectRoutine};
use x100_core::party::Actor;
use x100_core::task::TaskState;
use x100_core::transcript::{Item, TurnEnd};
use x100_core::wait::WaitEnd;

const HEADER: &str = r#"
[harness]
name = "fake-waits"

[[model]]
model = "fake-long"
context_window = 200000
cache_lifetime_secs = 604800
efforts = ["low", "medium", "high", "max"]
price = { input = 1.0, output = 5.0 }
"#;

fn script(turns: &str) -> String {
    format!("{HEADER}\n{turns}")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wait_past_its_deadline_wakes_the_agent_and_says_so() {
    // The job it waits for is ten hours off; its own deadline is in one.
    let script = script(
        r#"
[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Check the site" }
steps = [
  { call = { schedule = { when = { once = { at = 1794423600000 } }, does = { wake = { agent = "agt_00000000000000000000000009", note = "Look at the site again." } } } }, save = "later" },
  { call = { wait = { on = { job = "{{later.job}}" }, deadline = 1794391200000, task = "tsk_00000000000000000000000001", progress = { note = "Waiting for the site to come back.", done = 10 } } } },
]

[[turn]]
when = { input_contains = "ran out" }
steps = [
  { call = { cancel_job = { job = "{{later.job}}" } } },
  { call = { report = { task = "tsk_00000000000000000000000001", outcome = "completed", summary = "The site stayed down; gave up waiting." } } },
]
"#,
    );
    let w = World::new(Setup::new(&script)).await;
    let brief = w.brief("Check the site").await;
    w.settle().await;
    let s = w.state();
    assert!(matches!(
        s.agent(w.ceo).unwrap().state,
        AgentState::Waiting(_)
    ));
    let wait = s
        .agent(w.ceo)
        .unwrap()
        .waits
        .values()
        .next()
        .unwrap()
        .clone();

    // Just before the deadline nothing happens; at it, the agent is woken and told.
    w.clock_to(START + HOUR - std::time::Duration::from_millis(1))
        .await;
    w.settle().await;
    assert_eq!(w.state().agent(w.ceo).unwrap().waits.len(), 1);
    w.clock_to(START + HOUR).await;
    w.settle().await;

    let s = w.state();
    let log = w.log();
    let ended = log.iter().find_map(|e| match &e.event {
        ProjectEvent::Agent {
            event: AgentEvent::WaitEnded { wait: id, end },
            ..
        } if *id == wait.id => Some(*end),
        _ => None,
    });
    assert_eq!(ended, Some(WaitEnd::Expired));
    assert_eq!(s.job(wait.deadline_job).unwrap().state, JobState::Done);
    let told = log.iter().any(|e| matches!(&e.event,
        ProjectEvent::Agent { event: AgentEvent::Delivered { what: InboxItem::WaitEnded { wait: id, end: WaitEnd::Expired }, .. }, .. }
            if *id == wait.id));
    assert!(told, "the agent is told its wait ran out");
    // It read that its wait ran out, cancelled the job it no longer needs, and reported.
    let x100_core::wait::WaitOn::Job(later) = wait.on else {
        panic!("a wait on a job")
    };
    assert_eq!(
        s.job(later).unwrap().cancelled_by,
        Some(Actor::Agent(w.ceo))
    );
    assert!(matches!(s.task(brief).unwrap().state, TaskState::Done(..)));
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_whose_driver_is_stopped_during_a_long_wait_resumes_from_its_start_pack() {
    let script = script(
        r#"
[[turn]]
when = { start_pack_contains = "Progress note (25% done): Waiting for the morning check.", input_contains = "Time for the morning check." }
steps = [
  { call = { report = { task = "tsk_00000000000000000000000001", outcome = "completed", summary = "Checked in the morning." } } },
]

[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Morning check" }
steps = [
  { call = { schedule = { when = { once = { at = 1794394800000 } }, does = { wake = { agent = "agt_00000000000000000000000009", note = "Time for the morning check." } } } }, save = "morning" },
  { call = { wait = { on = { job = "{{morning.job}}" }, task = "tsk_00000000000000000000000001", progress = { note = "Waiting for the morning check.", done = 25 } } } },
]
"#,
    );
    let w = World::new(Setup::new(&script)).await;
    let brief = w.brief("Morning check").await;
    w.settle().await;
    let first = w.state().agent(w.ceo).unwrap().drivers[0];
    assert_eq!(w.state().driver(first).unwrap().state, DriverState::Warm);

    // The builder stops the warm driver. The agent keeps waiting, with no driver at all.
    w.orch.stop_driver(w.project, first).await.expect("stopped");
    w.settle().await;
    let s = w.state();
    assert_eq!(
        s.driver(first).unwrap().state,
        DriverState::Ended(EndReason::Stopped(StopReason::ReplacedByBuilder))
    );
    assert!(s.acting_driver(w.ceo).is_none());
    assert!(matches!(
        s.agent(w.ceo).unwrap().state,
        AgentState::Waiting(_)
    ));
    // Its warm period has nothing left to end.
    assert!(s.jobs().all(|j| !matches!(j.job.does,
        ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver }) if driver == first)
        || j.state == JobState::Cancelled));

    w.clock_to(START + 2 * HOUR).await;
    w.settle().await;
    let s = w.state();
    let drivers = &s.agent(w.ceo).unwrap().drivers;
    assert_eq!(drivers.len(), 2);
    let pack = w
        .orch
        .start_pack(w.project, drivers[1])
        .expect("a start pack");
    assert!(pack.contains("Waiting for the morning check."), "{pack}");
    assert!(pack.contains("was stopped (ReplacedByBuilder)"), "{pack}");
    assert!(pack.contains("DAEMON TOOL CALLS IN THIS TASK"), "{pack}");
    assert!(matches!(s.task(brief).unwrap().state, TaskState::Done(..)));
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_item_given_to_a_driver_stopped_before_its_turn_completes_reaches_the_next() {
    // The first driver goes quiet mid-turn. The second knows from its start pack why it started.
    let script = script(
        r#"
[[turn]]
when = { start_pack_contains = "was stopped (ReplacedByBuilder)" }
steps = [
  { call = { report = { task = "tsk_00000000000000000000000001", outcome = "completed", summary = "Done on the next driver." } } },
]

[[turn]]
when = { input_contains = "Long job" }
steps = [{ text = "Starting the long job." }, { stall = true }]
"#,
    );
    let w = World::new(Setup::new(&script)).await;
    let brief = w.brief("Long job").await;
    w.until("the first turn is running", |s| {
        s.drivers().any(|d| d.state == DriverState::Running)
    })
    .await;
    let s = w.state();
    let first = s.agent(w.ceo).unwrap().drivers[0];
    let item = s
        .agent(w.ceo)
        .unwrap()
        .inbox
        .unhandled()
        .next()
        .expect("the brief's item");
    assert!(
        s.agent(w.ceo).unwrap().inbox.waiting().next().is_none(),
        "it is held by the turn"
    );

    w.orch.stop_driver(w.project, first).await.expect("stopped");
    w.settle().await;

    let s = w.state();
    let drivers = s.agent(w.ceo).unwrap().drivers.clone();
    assert_eq!(drivers.len(), 2);
    let log = w.log();
    let given_to = |driver| {
        log.iter().any(|e| matches!(&e.event,
            ProjectEvent::Driver { driver: d, event: DriverEvent::TurnStarted { given } } if *d == driver && given.contains(&item)))
    };
    assert!(given_to(drivers[0]) && given_to(drivers[1]));
    assert!(
        s.agent(w.ceo).unwrap().inbox.is_empty(),
        "handled by the turn that completed"
    );
    assert!(matches!(s.task(brief).unwrap().state, TaskState::Done(..)));
    // The agent's transcript shows the first turn cut short and the change of driver.
    let entries = w.store.transcripts.read(w.ceo, None, 1000).unwrap();
    assert!(entries.iter().any(|(_, e)| e.driver == first
        && e.item
            == Item::TurnEnded {
                how: TurnEnd::Interrupted
            }));
    assert!(entries.iter().any(
        |(_, e)| e.driver == drivers[1] && e.item == Item::DriverChanged { from: Some(first) }
    ));
    w.finish().await;
}
