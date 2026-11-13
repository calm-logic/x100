//! M9's check, the takeovers, through the daemon as a child process on the fake harness posing as
//! two providers, on the fake clock with a seed:
//!
//! - A driver killed mid-task is replaced by one from the other provider at a compatible effort,
//!   which finishes the task. The worktree holds the first driver's changes. The new driver's
//!   start pack lists every daemon tool call the first made in the task, and each delegation,
//!   message and job exists once in the log. The agent's transcript is one record with the handoff
//!   marked.
//! - A driver killed with a daemon tool call in flight: the call takes effect once, and its result
//!   is in the start pack.
//! - A call from a replaced driver is refused.
//! - A stalled driver is replaced once the stall timeout passes.
//!
//! The three failures in a row and the daemon's restart are `handoff_restart.rs`; the planned
//! handoffs are `handoff_planned.rs`.

mod common;
#[allow(dead_code)]
#[path = "../../x100-orch/tests/common/mcp.rs"]
mod mcp;
#[path = "handoff/rig.rs"]
mod rig;

use std::time::Duration;

use rig::*;
use serde_json::json;
use x100_core::action::{ActionState, Request};
use x100_core::driver::{
    DriverState, Failure, HandoffCause, InFlight, StartReason, StopReason, TakeoverCause,
};
use x100_core::event::ProjectEvent;
use x100_core::job::ProjectJob;
use x100_core::task::{Outcome, TaskState};
use x100_core::tool::ToolCall;
use x100_core::transcript::Item;
use x100_proto::{ErrorCode, ReplaceDriver};

/// The CEO's task done by a driver that took over, as the scenario's word says.
fn finished_after_takeover(word: &str) -> String {
    format!(
        r#"
[[turn]]
when = {{ start_pack_contains = "You take over", input_contains = "A task for you: {word}" }}
repeat = true
steps = [
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "{word} finished after the takeover." }} }} }},
]
"#
    )
}

const KILL: &str = r#"
[[turn]]
when = { input_contains = "A task for you: KILL" }
steps = [
  { write = "first.txt", content = "written by the first driver" },
  { call = { delegate = { to = { new_worker = { role = "Helper", charter = "Helps." } }, task = { new = { title = "Help", ask = "Help out.", attachments = [] } }, effort = "low" } } },
  { call = { message = { to = "builder", body = "KILL-MESSAGE", attachments = [] } } },
  { call = { schedule = { when = { once = { at = 1795000000000 } }, does = { wake = { agent = "{{pack:Your id}}", note = "KILL-JOB" } } } } },
  { call = { update_task = { task = "{{input:Task}}", progress = { note = "KILL-NOTE: wrote first.txt, delegated, told the builder, scheduled a job.", done = 50 } } } },
  { run = "sleep 600" },
]

[[turn]]
when = { input_contains = "A task for you: Help" }
repeat = true
steps = [
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "Helped." } } },
]
"#;

fn takeover_of(why: &StartReason) -> (HandoffCause, InFlight) {
    let StartReason::Handoff(h) = why else {
        panic!("not a handoff: {why:?}")
    };
    (h.cause.clone(), h.in_flight.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_killed_driver_is_taken_over_by_the_other_provider_which_finishes_the_task() {
    let turns = format!(
        "{}{}{KILL}",
        set_up("KILL"),
        finished_after_takeover("KILL")
    );
    let rig = Rig::start("kill", &turns).await;
    let task = rig.brief("KILL").await;
    let ceo = rig.ceo;
    let first = rig.begins(ceo, "run").await;
    rig.kill(first);

    let report = rig
        .until("the CEO's task is reported", |p| {
            match &p.task(task)?.state {
                TaskState::InReview(_, r) | TaskState::Promoting(_, r) | TaskState::Done(_, r) => {
                    Some(r.clone())
                }
                _ => None,
            }
        })
        .await;
    assert_eq!(report.outcome, Outcome::Completed);
    assert_eq!(report.summary, "KILL finished after the takeover.");

    let drivers = rig.drivers_since(ceo, first);
    assert_eq!(drivers.len(), 2, "one takeover: {drivers:#?}");
    let (old, new) = (&drivers[0], &drivers[1]);
    assert_eq!(old.driver.id, first);
    assert!(
        matches!(
            old.state,
            DriverState::Failed(Failure::ProcessExited { .. })
        ),
        "{:?}",
        old.state
    );
    assert_eq!(rig.provider(old.driver.id), CHEAP);
    assert_eq!(rig.provider(new.driver.id), DEAR, "the other provider");
    let s = rig.state();
    let range = s.project().unwrap().team.effort(ceo).unwrap();
    let needs = s.task(task).unwrap().state.assignment().unwrap().effort;
    assert!(
        range.admits(new.driver.effort, needs),
        "{:?} in {range:?} for {needs:?}",
        new.driver.effort
    );
    let StartReason::Handoff(h) = &new.driver.why else {
        panic!(
            "the new driver's record names no handoff: {:?}",
            new.driver.why
        )
    };
    assert_eq!(h.replaces, first);
    assert!(matches!(
        h.cause,
        HandoffCause::Takeover(TakeoverCause::Failed(Failure::ProcessExited { .. }))
    ));
    assert_eq!(
        h.in_flight,
        InFlight::Command {
            command: "sleep 600".into()
        }
    );

    // The worktree holds the first driver's work, checkpointed once it could change it no more.
    let file = rig.worktree(ceo).join("first.txt");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "written by the first driver"
    );
    let log = rig.log().await;
    let at = |f: &dyn Fn(&ProjectEvent) -> bool| log.iter().position(|e| f(&e.event));
    let failed = at(&|e| {
        matches!(e, ProjectEvent::Driver { driver, event: x100_core::driver::DriverEvent::Failed(_) } if *driver == first)
    })
    .expect("the failure is in the log");
    let started = at(&|e| matches!(e, ProjectEvent::DriverStarted(d) if d.id == new.driver.id))
        .expect("the new driver is in the log");
    assert!(
        log[failed..started].iter().any(|e| matches!(&e.event,
            ProjectEvent::Checkpointed { agent, .. } if *agent == ceo)),
        "the worktree is checkpointed between the failure and the new driver"
    );

    // The start pack lists every daemon call the first driver made in the task, and what it was
    // running.
    let pack = rig.start_pack(new.driver.id).await;
    let calls = pack
        .split("DAEMON TOOL CALLS IN THIS TASK")
        .nth(1)
        .expect("the calls are listed");
    for name in ["- delegate ", "- message ", "- schedule ", "- update_task "] {
        assert!(calls.contains(name), "{name} in\n{pack}");
    }
    assert!(pack.contains("KILL-NOTE"), "{pack}");
    assert!(
        pack.contains(&format!(
            "You take over the work of your previous driver, {first}"
        )),
        "{pack}"
    );
    assert!(pack.contains("its process was killed"), "{pack}");
    assert!(
        pack.contains("`sleep 600`") && pack.contains("outcome is unknown"),
        "{pack}"
    );

    // Each delegation, message and job exists once.
    let count = |f: &dyn Fn(&ProjectEvent) -> bool| log.iter().filter(|e| f(&e.event)).count();
    assert_eq!(
        count(&|e| matches!(e, ProjectEvent::TaskOpened { task, .. } if task.title == "Help")),
        1
    );
    assert_eq!(
        count(&|e| matches!(e, ProjectEvent::MessageSent(m) if m.body == "KILL-MESSAGE")),
        1
    );
    assert_eq!(
        count(&|e| matches!(e, ProjectEvent::JobScheduled(j)
            if matches!(&j.does, ProjectJob::Wake { note, .. } if note == "KILL-JOB"))),
        1
    );

    // One transcript: the first driver's entries, the change marked, then the second's.
    let transcript = rig.transcript(ceo).await;
    let mut seen = vec![];
    for e in &transcript {
        if seen.last() != Some(&e.driver) {
            seen.push(e.driver);
        }
    }
    assert!(
        seen.ends_with(&[first, new.driver.id]),
        "one record, the first driver's entries and then the second's: {seen:?}"
    );
    assert!(
        transcript
            .iter()
            .any(|e| e.driver == new.driver.id
                && e.item == Item::DriverChanged { from: Some(first) })
    );
    assert!(transcript.iter().any(|e| e.driver == new.driver.id
        && matches!(&e.item, Item::StartPack { text } if text.contains("You take over"))));
    rig.finish().await;
}

const INFLIGHT: &str = r#"
[[turn]]
when = { input_contains = "A task for you: INFLIGHT" }
steps = [
  { call = { message = { to = "builder", body = "INFLIGHT-MESSAGE", attachments = [] } }, crash = 9 },
]
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_call_in_flight_when_its_driver_dies_takes_effect_once() {
    let turns = format!(
        "{}{}{INFLIGHT}",
        set_up("INFLIGHT"),
        finished_after_takeover("INFLIGHT")
    );
    let rig = Rig::start("inflight", &turns).await;
    let task = rig.brief("INFLIGHT").await;
    let ceo = rig.ceo;
    rig.until("the task is reported", |p| {
        (!matches!(p.task(task)?.state, TaskState::InProgress(_))).then_some(())
    })
    .await;

    let crashed = rig
        .drivers_of(ceo)
        .into_iter()
        .find(|d| matches!(d.state, DriverState::Failed(_)))
        .expect("the driver that made the call is gone");
    let drivers = rig.drivers_since(ceo, crashed.driver.id);
    assert_eq!(drivers.len(), 2, "{drivers:#?}");
    assert_eq!(
        drivers[0].state,
        DriverState::Failed(Failure::ProcessExited { code: Some(9) })
    );
    let (cause, in_flight) = takeover_of(&drivers[1].driver.why);
    assert!(matches!(
        cause,
        HandoffCause::Takeover(TakeoverCause::Failed(Failure::ProcessExited {
            code: Some(9)
        }))
    ));
    let InFlight::ToolCall { action } = in_flight else {
        panic!("the call is in flight: {in_flight:?}")
    };
    let s = rig.state();
    let record = s.action(action).expect("the call's action");
    assert_eq!(record.state, ActionState::CarriedOut);
    assert!(matches!(&record.action.request,
        Request::Tool { driver, call: ToolCall::Message { body, .. } } if *driver == drivers[0].driver.id && body == "INFLIGHT-MESSAGE"));

    let log = rig.log().await;
    let sent: Vec<_> = log
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::MessageSent(m) if m.body == "INFLIGHT-MESSAGE" => Some(m.id),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "the call took effect once");
    let pack = rig.start_pack(drivers[1].driver.id).await;
    assert!(
        pack.contains("It was making a daemon tool call when it stopped, message"),
        "{pack}"
    );
    assert!(
        pack.contains(&sent[0].to_string()),
        "the call's result, the message's id, is in the pack:\n{pack}"
    );
    rig.finish().await;
}

const REPLACED: &str = r#"
[[turn]]
when = { input_contains = "A task for you: REPLACED" }
steps = [
  { write = ".x100/endpoint", content = "{{tools:url}} {{tools:token}}" },
  { run = "sleep 600" },
]
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_call_from_a_replaced_driver_is_refused() {
    let turns = format!(
        "{}{}{REPLACED}",
        set_up("REPLACED"),
        finished_after_takeover("REPLACED")
    );
    let rig = Rig::start("replaced", &turns).await;
    let task = rig.brief("REPLACED").await;
    let ceo = rig.ceo;
    let first = rig.begins(ceo, "run").await;
    let endpoint = std::fs::read_to_string(rig.worktree(ceo).join(".x100/endpoint")).unwrap();
    let (url, token) = endpoint.split_once(' ').expect("an address and a token");

    rig.client
        .request(ReplaceDriver {
            project: rig.project,
            agent: ceo,
        })
        .await
        .expect("the builder replaces the driver");
    rig.until("the task is reported", |p| {
        (!matches!(p.task(task)?.state, TaskState::InProgress(_))).then_some(())
    })
    .await;

    // The replaced driver, slow to die, calls with its own address and token.
    let stale = mcp::Client::at(url, token);
    let answer = stale
        .call(
            "message",
            json!({ "to": "builder", "body": "STALE-MESSAGE", "attachments": [] }),
        )
        .await;
    assert!(answer.is_error, "{answer:?}");
    assert!(
        answer.text.contains("stopped acting for its agent"),
        "{answer:?}"
    );
    assert!(
        rig.log().await.iter().all(|e| !matches!(&e.event,
            ProjectEvent::MessageSent(m) if m.body == "STALE-MESSAGE")),
        "a refused call changes nothing"
    );

    let drivers = rig.drivers_since(ceo, first);
    assert_eq!(
        drivers[0].state,
        DriverState::Ended(x100_core::driver::EndReason::Stopped(
            StopReason::ReplacedByBuilder
        ))
    );
    let (cause, in_flight) = takeover_of(&drivers[1].driver.why);
    assert_eq!(
        cause,
        HandoffCause::Takeover(TakeoverCause::Stopped(StopReason::ReplacedByBuilder))
    );
    assert_eq!(
        in_flight,
        InFlight::Command {
            command: "sleep 600".into()
        }
    );

    // An agent that does not exist has no driver to replace.
    let none = rig
        .client
        .request(ReplaceDriver {
            project: rig.project,
            agent: x100_core::id::AgentId::from_u128(404),
        })
        .await
        .unwrap_err();
    assert!(matches!(none, x100_client::ClientError::Rpc(e) if e.code == ErrorCode::NotFound));
    rig.finish().await;
}

const STALL: &str = r#"
[[turn]]
when = { input_contains = "A task for you: STALL" }
steps = [{ text = "going quiet" }, { stall = true }]
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_driver_is_replaced_once_the_stall_timeout_passes() {
    let turns = format!(
        "{}{}{STALL}",
        set_up("STALL"),
        finished_after_takeover("STALL")
    );
    let rig = Rig::start("stall", &turns).await;
    let task = rig.brief("STALL").await;
    let ceo = rig.ceo;
    let quiet = Item::Text {
        text: "going quiet".into(),
    };
    let first = rig.wrote(ceo, &quiet).await;
    let timeout = rig.client.with_state(|s| s.daemon.settings().stall_timeout);

    // Just short of the timeout, after several of the daemon's checks, it is still the driver.
    rig.clock_to(START + timeout - Duration::from_secs(1)).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        rig.state().driver(first).unwrap().state,
        DriverState::Running,
        "not replaced before the timeout"
    );

    rig.clock_to(START + timeout + Duration::from_secs(1)).await;
    rig.until("the task is reported", |p| {
        (!matches!(p.task(task)?.state, TaskState::InProgress(_))).then_some(())
    })
    .await;
    let drivers = rig.drivers_since(ceo, first);
    assert!(
        matches!(
            drivers[0].state,
            DriverState::Failed(Failure::Stalled { .. })
        ),
        "{:?}",
        drivers[0].state
    );
    let (cause, _) = takeover_of(&drivers[1].driver.why);
    assert!(matches!(
        cause,
        HandoffCause::Takeover(TakeoverCause::Failed(Failure::Stalled { .. }))
    ));
    assert_eq!(rig.provider(drivers[1].driver.id), DEAR);
    rig.finish().await;
}
