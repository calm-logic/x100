//! The router choosing for real: the plan's M8 check, with the fake harness posing as two
//! providers at different prices, the orchestrator in this process, and the fake clock.
//!
//! The builder names each piece of work in a message ("Start: first", "Level: low") and the CEO
//! hands it to its worker Dev, or to its leads. A title says what the worker's turn does: `big-`
//! fills its context, `costly-` spends two dollars, `flaky-` fails at the cheap provider.

mod common;

use std::time::Duration;

use common::*;
use x100_core::agent::{AgentEvent, AgentState, Waiting};
use x100_core::driver::{
    DriverEvent, DriverState, EndReason, Incompatible, PlannedReason, RetireReason,
};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::ProjectEvent;
use x100_core::fold::{DriverRecord, ProjectState};
use x100_core::harness::{HarnessModel, ModelName};
use x100_core::id::AgentId;
use x100_core::inbox::InboxItem;
use x100_core::job::{JobState, ProjectJob, ProjectRoutine};
use x100_core::ledger::BudgetReached;
use x100_core::message::Route;
use x100_core::money::Money;
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_core::task::TaskState;

const CHEAP: &str = r#"
[harness]
name = "fake-cheap"

[[model]]
model = "cheap"
context_window = 200000
cache_lifetime_secs = 300
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { start_pack_contains = "ROLE: Dev\n", input_contains = "flaky-" }
steps = [{ text = "Calling the model." }, { fail = "rate_limited", detail = "slow down" }]
"#;

const DEAR: &str = r#"
[harness]
name = "fake-dear"

[[model]]
model = "dear"
context_window = 200000
cache_lifetime_secs = 300
efforts = ["low", "medium", "high", "max"]
price = { input = 5.0, output = 25.0 }
"#;

const TURNS: &str = r#"
[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Start: " }
steps = [
  { call = { delegate = { to = { new_worker = { role = "Dev", charter = "Does the work." } }, task = { new = { title = "{{input:Start}}", ask = "As named.", attachments = [] } }, effort = "{{input:Level}}" } } },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "Handed to Dev." } } },
]

[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Next: " }
repeat = true
steps = [
  { call = { delegate = { to = { child = "{{pack:Worker Dev}}" }, task = { new = { title = "{{input:Next}}", ask = "As named.", attachments = [] } }, effort = "{{input:Level}}" } } },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "Handed to Dev." } } },
]

[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Team: " }
steps = [
  { call = { form_team = { leads = [
      { role = "Tech", charter = "Tech.", effort = { min = "low", max = "max" }, budget = 1000000 },
      { role = "Product", charter = "Product.", effort = { min = "low", max = "max" }, budget = 5000000 },
    ] } }, save = "team" },
  { call = { delegate = { to = { child = "{{team.Tech}}" }, task = { new = { title = "costly-a", ask = "a", attachments = [] } }, effort = "low" } } },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "Team formed." } } },
]

[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Both: " }
steps = [
  { call = { delegate = { to = { child = "{{pack:Lead Tech}}" }, task = { new = { title = "costly-b", ask = "b", attachments = [] } }, effort = "low" } } },
  { call = { delegate = { to = { child = "{{pack:Lead Product}}" }, task = { new = { title = "spec-b", ask = "b", attachments = [] } }, effort = "low" } } },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "Handed on." } } },
]

[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Report on: " }
repeat = true
steps = [{ call = { review = { task = "{{input:Report on}}", verdict = "accept" } } }]

# Anything else the CEO is told, such as an agent waiting for a driver, it notes.
[[turn]]
when = { start_pack_contains = "ROLE: CEO\n" }
repeat = true
steps = [{ text = "Noted." }]

[[turn]]
when = { input_contains = "big-" }
repeat = true
steps = [
  { context = 150000 },
  { call = { update_task = { task = "{{input:Task}}", progress = { note = "Half of the big job is done.", done = 50 } } } },
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "The big job is done." } } },
]

[[turn]]
when = { input_contains = "costly-" }
repeat = true
steps = [
  { usage = { input = 10, output = 1, cost_micros = 2000000 } },
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Spent a lot." } } },
]

[[turn]]
when = { input_contains = "Task: " }
repeat = true
steps = [
  { context = 10000 },
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Done." } } },
]
"#;

fn cheap() -> String {
    format!("{CHEAP}{TURNS}")
}

fn dear() -> String {
    format!("{DEAR}{TURNS}")
}

fn on(w: &World, provider: usize, model: &str) -> HarnessModel {
    HarnessModel {
        harness: w.harnesses[provider],
        model: ModelName(model.into()),
    }
}

async fn world(allowed: Option<&[(usize, &str)]>, budget: Money) -> World {
    let (cheap, dear) = (cheap(), dear());
    let providers = [dear.as_str()];
    let mut setup = Setup::new(&cheap);
    setup.providers = &providers;
    setup.allowed = allowed;
    setup.budget = budget;
    World::new(setup).await
}

async fn say(w: &World, body: &str) {
    w.orch
        .send_chat(w.project, body.into(), vec![], None)
        .await
        .expect("the message is sent");
}

fn drivers(s: &ProjectState, agent: AgentId) -> Vec<&DriverRecord> {
    s.agent(agent)
        .unwrap()
        .drivers
        .iter()
        .filter_map(|d| s.driver(*d))
        .collect()
}

fn of(d: &DriverRecord) -> (HarnessModel, Effort) {
    let model = HarnessModel {
        harness: d.driver.harness,
        model: d.driver.model.clone(),
    };
    (model, d.driver.effort)
}

fn done(s: &ProjectState, title: &str) -> bool {
    s.tasks()
        .any(|t| t.task.title == title && matches!(t.state, TaskState::Done(..)))
}

fn waiting_for_driver(s: &ProjectState, agent: AgentId) -> Option<Vec<Incompatible>> {
    match &s.agent(agent)?.state {
        AgentState::Waiting(Waiting::ForDriver(why)) => Some(why.clone()),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_arriving_while_a_compatible_driver_is_warm_reuses_it() {
    let w = world(None, Money::from_dollars(50)).await;
    w.brief("Start: first\nLevel: low").await;
    w.until("the first task is done", |s| done(s, "first"))
        .await;
    w.settle().await;
    say(&w, "Next: second\nLevel: low").await;
    w.until("the second task is done", |s| done(s, "second"))
        .await;
    let s = w.state();
    let dev = agent(&s, "Dev");
    let ds = drivers(&s, dev);
    assert_eq!(ds.len(), 1, "one driver did both tasks");
    assert_eq!(of(ds[0]), (on(&w, 0, "cheap"), Effort::Low), "the cheapest");
    assert_eq!(ds[0].turns, 2);
    // Its first task was accepted, so it compacted before it took the second.
    let id = ds[0].driver.id;
    let log = w.log();
    let compacted = log.iter().position(|e| {
        matches!(&e.event, ProjectEvent::Driver { driver, event: DriverEvent::Compacted } if *driver == id)
    });
    let second_turn = log
        .iter()
        .filter(|e| {
            matches!(&e.event, ProjectEvent::Driver { driver, event: DriverEvent::TurnStarted { .. } } if *driver == id)
        })
        .nth(1)
        .map(|e| e.seq.0 as usize - 1);
    assert!(
        compacted.is_some() && compacted < second_turn,
        "compacted before"
    );
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_arriving_after_the_warm_period_gets_a_new_driver() {
    let w = world(None, Money::from_dollars(50)).await;
    w.brief("Start: first\nLevel: low").await;
    w.until("the first task is done", |s| done(s, "first"))
        .await;
    w.settle().await;
    // The cheap model keeps its cache five minutes.
    w.clock_to(START + Duration::from_secs(600)).await;
    let dev = agent(&w.state(), "Dev");
    w.until("the warm driver is retired", |s| {
        drivers(s, dev)[0].state
            == DriverState::Ended(EndReason::Retired(RetireReason::WarmTooLong))
    })
    .await;
    say(&w, "Next: second\nLevel: low").await;
    w.until("the second task is done", |s| done(s, "second"))
        .await;
    let s = w.state();
    let ds = drivers(&s, dev);
    assert_eq!(ds.len(), 2, "a new driver for the second task");
    assert_eq!(of(ds[1]), (on(&w, 0, "cheap"), Effort::Low));
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_needing_more_effort_than_the_warm_driver_has_gets_a_new_driver() {
    let w = world(None, Money::from_dollars(50)).await;
    w.brief("Start: first\nLevel: low").await;
    w.until("the first task is done", |s| done(s, "first"))
        .await;
    w.settle().await;
    say(&w, "Next: second\nLevel: max").await;
    w.until("the second task is done", |s| done(s, "second"))
        .await;
    let s = w.state();
    let ds = drivers(&s, agent(&s, "Dev"));
    assert_eq!(ds.len(), 2);
    assert_eq!(
        ds[0].state,
        DriverState::Ended(EndReason::HandedOff(PlannedReason::EffortChanged))
    );
    // Only the dear provider runs at Max.
    assert_eq!(of(ds[1]), (on(&w, 1, "dear"), Effort::Max));
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_past_the_context_threshold_between_tasks_is_retired_and_its_successor_has_what_it_needed()
 {
    let w = world(None, Money::from_dollars(50)).await;
    w.brief("Start: big-one\nLevel: low").await;
    let dev = |s: &ProjectState| s.agents().find(|a| a.role == "Dev").map(|a| a.id);
    w.until("the full driver is retired", |s| {
        dev(s).is_some_and(|d| {
            drivers(s, d).first().is_some_and(|r| {
                r.state == DriverState::Ended(EndReason::Retired(RetireReason::ContextFull))
            })
        })
    })
    .await;
    w.until("the big task is done", |s| done(s, "big-one"))
        .await;
    w.settle().await;
    say(&w, "Next: second\nLevel: low").await;
    w.until("the second task is done", |s| done(s, "second"))
        .await;
    let s = w.state();
    let ds = drivers(&s, dev(&s).unwrap());
    assert_eq!(ds.len(), 2);
    let pack = w
        .orch
        .start_pack(w.project, ds[1].driver.id)
        .expect("the new driver's start pack");
    assert!(pack.contains("YOUR RECENT TASKS"), "{pack}");
    assert!(pack.contains("\"big-one\""), "{pack}");
    assert!(pack.contains("Report: The big job is done."), "{pack}");
    assert!(
        pack.contains("Last note: Half of the big job is done."),
        "{pack}"
    );
    assert!(pack.contains("retired (ContextFull)"), "{pack}");
    w.finish().await;
}

/// Only the cheap model is allowed, and it does not run at Max.
async fn waiting_for_max() -> (World, AgentId) {
    let allowed: &[(usize, &str)] = &[(0, "cheap")];
    let w = world(Some(allowed), Money::from_dollars(50)).await;
    w.brief("Start: first\nLevel: max").await;
    let dev = |s: &ProjectState| s.agents().find(|a| a.role == "Dev").map(|a| a.id);
    w.until("the worker waits for a driver", |s| {
        dev(s).is_some_and(|d| waiting_for_driver(s, d).is_some())
    })
    .await;
    w.settle().await;
    let s = w.state();
    let dev = dev(&s).unwrap();
    assert_eq!(
        waiting_for_driver(&s, dev).unwrap(),
        vec![Incompatible::NoEffortInRange, Incompatible::NothingAllowed],
        "every rule that excluded something says so"
    );
    assert!(drivers(&s, dev).is_empty());
    // Its parent was told why.
    let told = w.log().iter().any(|e| {
        matches!(&e.event, ProjectEvent::Agent { agent, event: AgentEvent::Delivered { what: InboxItem::NoDriver { agent: a, .. }, .. } }
            if *agent == s.project().unwrap().ceo && *a == dev)
    });
    assert!(told, "the CEO is told");
    // The wait has a deadline.
    let deadline = s.jobs().any(|j| {
        matches!(&j.job.does, ProjectJob::Routine(ProjectRoutine::WaitDeadline { agent, .. }) if *agent == dev)
            && matches!(j.state, JobState::Scheduled { .. })
    });
    assert!(deadline, "the wait for a driver has a deadline");
    (w, dev)
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_compatible_the_agent_waits_its_parent_is_told_and_it_starts_when_a_setting_changes()
 {
    let (w, dev) = waiting_for_max().await;
    let allow = SettingChange::Allow {
        what: on(&w, 1, "dear"),
        allowed: true,
    };
    w.store
        .log
        .append(Actor::Builder, ProjectEvent::SettingChanged(allow))
        .unwrap();
    w.until("the task is done", |s| done(s, "first")).await;
    let s = w.state();
    let ds = drivers(&s, dev);
    assert_eq!(ds.len(), 1);
    assert_eq!(of(ds[0]), (on(&w, 1, "dear"), Effort::Max));
    let found = w.log().iter().any(|e| {
        matches!(&e.event, ProjectEvent::Agent { agent, event: AgentEvent::DriverFound } if *agent == dev)
    });
    assert!(found);
    let open_deadline = s.jobs().any(|j| {
        matches!(&j.job.does, ProjectJob::Routine(ProjectRoutine::WaitDeadline { agent, .. }) if *agent == dev)
            && matches!(j.state, JobState::Scheduled { .. })
    });
    assert!(!open_deadline, "its deadline is called off");
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_a_range_changes_the_next_decision() {
    let (w, dev) = waiting_for_max().await;
    let range = EffortRange::new(Effort::Low, Effort::High).unwrap();
    w.orch.set_effort(w.project, dev, range).await.unwrap();
    w.until("the task is done", |s| done(s, "first")).await;
    let s = w.state();
    // The task asked for Max; inside the new range that is High, which the cheap model runs.
    assert_eq!(of(drivers(&s, dev)[0]), (on(&w, 0, "cheap"), Effort::High));
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wait_for_a_driver_past_its_deadline_reports_the_task_failed() {
    let (w, dev) = waiting_for_max().await;
    w.clock_to(START + 8 * 24 * HOUR).await;
    w.until("the task is reported", |s| {
        s.tasks()
            .any(|t| t.task.title == "first" && !matches!(t.state, TaskState::InProgress(_)))
    })
    .await;
    let s = w.state();
    let task = s.tasks().find(|t| t.task.title == "first").unwrap();
    let (TaskState::InReview(_, report)
    | TaskState::Promoting(_, report)
    | TaskState::Done(_, report)) = &task.state
    else {
        panic!("reported");
    };
    assert!(matches!(
        &report.outcome,
        x100_core::task::Outcome::Failed(x100_core::task::TaskFailure::NoDriver(_))
    ));
    assert_eq!(s.agent(dev).unwrap().state, AgentState::Idle);
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lead_at_its_budget_starts_nothing_while_other_leads_carry_on() {
    let w = world(None, Money::from_dollars(50)).await;
    w.brief("Team: Tech and Product").await;
    w.until("Tech's first task is done", |s| done(s, "costly-a"))
        .await;
    w.settle().await;
    let s = w.state();
    let (ceo, tech, product) = (
        s.project().unwrap().ceo,
        agent(&s, "Tech"),
        agent(&s, "Product"),
    );
    let log = w.log();
    assert!(log.iter().any(|e| matches!(
        &e.event,
        ProjectEvent::BudgetReached(BudgetReached::Lead(l)) if *l == tech
    )));
    assert!(log.iter().any(|e| matches!(&e.event,
        ProjectEvent::Agent { agent, event: AgentEvent::Delivered { what: InboxItem::BudgetReached { lead }, .. } }
            if *agent == ceo && *lead == tech)), "the CEO is told");

    say(&w, "Both: one each").await;
    w.until("Product's task is done", |s| done(s, "spec-b"))
        .await;
    w.until("Tech waits for a driver", |s| {
        waiting_for_driver(s, tech).is_some()
    })
    .await;
    let s = w.state();
    assert_eq!(
        waiting_for_driver(&s, tech).unwrap(),
        vec![Incompatible::Budget(BudgetReached::Lead(tech))]
    );
    assert_eq!(
        drivers(&s, tech).iter().map(|d| d.turns).sum::<u32>(),
        1,
        "no second turn"
    );
    assert!(!drivers(&s, product).is_empty());

    // The builder raises Tech's budget, and the next decision lets it start.
    w.orch
        .set_budget(w.project, tech, Money::from_dollars(4))
        .await
        .unwrap();
    w.until("Tech's second task is done", |s| done(s, "costly-b"))
        .await;
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_at_its_total_starts_nothing() {
    let w = world(None, Money::from_dollars(1)).await;
    w.brief("Start: costly-x\nLevel: low").await;
    w.until("the project reaches its total", |s| {
        s.budget().is_some_and(|b| b.project.stopped)
    })
    .await;
    let ceo = w.ceo;
    w.until("the CEO waits for a driver", |s| {
        waiting_for_driver(s, ceo).is_some()
    })
    .await;
    w.settle().await;
    let before = w.state().drivers().count();
    say(&w, "Next: more\nLevel: low").await;
    w.settle().await;
    let s = w.state();
    assert_eq!(s.drivers().count(), before, "nothing new starts");
    assert_eq!(
        waiting_for_driver(&s, ceo).unwrap(),
        vec![Incompatible::Budget(BudgetReached::Project)]
    );
    let notices: Vec<_> = s
        .messages()
        .filter(|m| m.message.route == Route::DaemonToBuilder)
        .map(|m| m.message.body.clone())
        .collect();
    assert!(
        notices.iter().any(|b| b.contains("has reached its budget")),
        "the daemon posts the notice in the chat: {notices:?}"
    );
    assert_eq!(
        w.log()
            .iter()
            .filter(|e| matches!(e.event, ProjectEvent::BudgetReached(BudgetReached::Project)))
            .count(),
        1
    );
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_harness_and_model_that_fail_with_a_provider_error_are_skipped_until_their_cool_off_ends()
{
    let w = world(None, Money::from_dollars(50)).await;
    w.brief("Start: flaky-one\nLevel: low").await;
    w.until("the task is done", |s| done(s, "flaky-one")).await;
    w.settle().await;
    let cheap = on(&w, 0, "cheap");
    let cooling = w.daemon.with_state(|d| d.cooling_off(&cheap).cloned());
    let cooling = cooling.expect("the cheap model cools off for every project");
    let s = w.state();
    let dev = agent(&s, "Dev");
    let ds = drivers(&s, dev);
    assert_eq!(ds.len(), 2);
    assert!(matches!(ds[0].state, DriverState::Failed(_)));
    assert_eq!(
        of(ds[1]),
        (on(&w, 1, "dear"), Effort::Low),
        "the dear one meanwhile"
    );

    // Still cooling off: a new task stays off the cheap model.
    w.clock_to(START + Duration::from_secs(400)).await;
    say(&w, "Next: during\nLevel: low").await;
    w.until("the task is done", |s| done(s, "during")).await;
    w.settle().await;
    let s = w.state();
    let last = drivers(&s, dev).last().map(|d| of(d).0).unwrap();
    assert_eq!(last, on(&w, 1, "dear"));

    // Once its job ends the cool-off, the cheap model is chosen again.
    w.clock_to(cooling.cool_off.until + Duration::from_secs(600))
        .await;
    assert!(w.daemon.with_state(|d| d.cooling_off(&cheap).is_none()));
    w.settle().await;
    say(&w, "Next: after\nLevel: low").await;
    w.until("the task is done", |s| done(s, "after")).await;
    let s = w.state();
    let last = drivers(&s, dev).last().map(|d| of(d)).unwrap();
    assert_eq!(last, (cheap, Effort::Low));
    w.finish().await;
}
