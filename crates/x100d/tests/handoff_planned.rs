//! M9's check, the planned handoffs, through the daemon as a child process on the fake harness
//! posing as two providers, on the fake clock with a seed: for each planned reason, a healthy
//! driver whose agent is mid-task hands off after a last turn that brings its progress note up to
//! date, and the next driver starts from that note.
//!
//! Each scenario's first driver works on the CEO's task and ends its turn mid-task. Two reasons
//! come from the turn itself: the context it reports, and the effort it says the task needs. The
//! other two come from the builder while the turn runs: the turn waits on a file the test writes
//! once the change is made.

mod common;
#[path = "handoff/rig.rs"]
mod rig;

use rig::*;
use x100_core::action::Request;
use x100_core::driver::{
    DriverEvent, DriverState, EndReason, HandoffCause, InFlight, PlannedReason, StartReason,
};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::ProjectEvent;
use x100_core::harness::{HarnessModel, ModelName};
use x100_core::id::TaskId;
use x100_core::project::SettingChange;
use x100_core::task::TaskState;
use x100_core::tool::ToolCall;

/// The scenario's turns: `work` is the first driver's turn on the task, which ends mid-task.
fn turns(word: &str, work: &str) -> String {
    format!(
        r#"{set_up}
[[turn]]
when = {{ input_contains = "HAND OFF" }}
repeat = true
steps = [
  {{ call = {{ update_task = {{ task = "{{{{input:Task}}}}", progress = {{ note = "{word}-HANDOFF-NOTE: the work is half done; next, finish it.", done = 60 }} }} }} }},
]

[[turn]]
when = {{ start_pack_contains = "{word}-HANDOFF-NOTE" }}
repeat = true
steps = [
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "{word} finished from the note." }} }} }},
]

[[turn]]
when = {{ input_contains = "A task for you: {word}" }}
steps = [
  {{ call = {{ update_task = {{ task = "{{{{input:Task}}}}", progress = {{ note = "{word}-EARLY-NOTE", done = 20 }} }} }} }},
{work}
]
"#,
        set_up = set_up(word)
    )
}

/// What the builder does while a turn runs.
type Change<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>;

/// The turn waits for the test to write `release` in its worktree.
const HELD: &str = r#"  { run = "while [ ! -e release ]; do sleep 0.05; done; rm -f release" },"#;

/// Runs a scenario through its handoff and checks everything every planned reason shares.
/// `change` is what the builder does while the first driver's turn runs, if anything.
async fn planned(
    word: &str,
    work: &str,
    reason: PlannedReason,
    change: Option<fn(&Rig) -> Change<'_>>,
) -> (Rig, TaskId, x100_core::fold::DriverRecord) {
    let rig = Rig::start(&word.to_lowercase(), &turns(word, work)).await;
    // The whole range, so a task can come to need more than its first driver runs at.
    rig.client
        .set_effort_range(rig.project, rig.ceo, EffortRange::FULL)
        .await
        .unwrap();
    let task = rig.brief(word).await;
    let ceo = rig.ceo;
    if let Some(change) = change {
        let held = rig.begins(ceo, "run").await;
        change(&rig).await;
        std::fs::write(rig.worktree(ceo).join("release"), "").unwrap();
        assert_eq!(
            rig.state().driver(held).unwrap().state,
            DriverState::Running
        );
    }
    rig.until("the task is reported", |p| {
        (!matches!(p.task(task)?.state, TaskState::InProgress(_))).then_some(())
    })
    .await;

    let log = rig.log().await;
    let started = log
        .iter()
        .position(|e| matches!(&e.event, ProjectEvent::Driver { event: DriverEvent::HandoffStarted(r), .. } if *r == reason))
        .unwrap_or_else(|| {
            let drivers: Vec<_> = rig
                .drivers_of(ceo)
                .into_iter()
                .map(|d| (d.driver.effort, d.driver.why, d.state))
                .collect();
            let task = rig.state().task(task).cloned();
            panic!("no planned handoff began: {drivers:#?}\n{task:#?}")
        });
    let ProjectEvent::Driver { driver: old, .. } = log[started].event else {
        unreachable!()
    };
    let note = log
        .iter()
        .position(|e| matches!(&e.event, ProjectEvent::ActionProposed(a)
            if matches!(&a.request, Request::Tool { driver, call: ToolCall::UpdateTask { progress, .. } }
                if *driver == old && progress.note.starts_with(&format!("{word}-HANDOFF-NOTE")))))
        .expect("the old driver's last turn writes the note");
    let ended = log
        .iter()
        .enumerate()
        .skip(note)
        .find_map(|(i, e)| {
            matches!(&e.event, ProjectEvent::Driver { driver, event: DriverEvent::TurnEnded } if *driver == old)
                .then_some(i)
        })
        .expect("the last turn ends");
    let new = rig
        .drivers_since(ceo, old)
        .get(1)
        .cloned()
        .expect("a new driver");
    let new_started = log
        .iter()
        .position(|e| matches!(&e.event, ProjectEvent::DriverStarted(d) if d.id == new.driver.id))
        .unwrap();
    assert!(
        started < note && note < ended && ended < new_started,
        "the handoff begins, the note is written, the old driver ends, then the new one starts"
    );
    assert_eq!(
        rig.state().driver(old).unwrap().state,
        DriverState::Ended(EndReason::HandedOff(reason))
    );
    let StartReason::Handoff(h) = &new.driver.why else {
        panic!(
            "the new driver's record names the handoff: {:?}",
            new.driver.why
        )
    };
    assert_eq!(h.replaces, old);
    assert_eq!(h.cause, HandoffCause::Planned(reason));
    assert_eq!(h.in_flight, InFlight::Nothing);
    let pack = rig.start_pack(new.driver.id).await;
    assert!(
        pack.contains(&format!(
            "Progress note (60% done): {word}-HANDOFF-NOTE: the work is half done"
        )),
        "{pack}"
    );
    assert!(pack.contains("You take over"), "{pack}");
    (rig, task, new)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_whose_context_fills_mid_task_hands_off() {
    let work = r#"  { context = 190000 },"#;
    let (rig, _, _) = planned("CONTEXT", work, PlannedReason::ContextFull, None).await;
    rig.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_below_its_tasks_new_effort_hands_off() {
    let work = r#"  { call = { update_task = { task = "{{input:Task}}", progress = { note = "EFFORT-MORE: this needs more.", done = 30 }, effort = "max" } } },"#;
    let (rig, task, new) = planned("EFFORT", work, PlannedReason::EffortChanged, None).await;
    let needs = rig
        .state()
        .task(task)
        .unwrap()
        .state
        .assignment()
        .unwrap()
        .effort;
    assert_eq!(needs, Effort::Max);
    assert_eq!(new.driver.effort, Effort::Max);
    rig.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_whose_model_the_builder_disallows_hands_off() {
    fn disallow(rig: &Rig) -> Change<'_> {
        Box::pin(async move {
            let harness = rig.client.with_state(|s| {
                s.daemon
                    .harnesses()
                    .find(|h| {
                        h.kind == x100_core::harness::HarnessKind::Custom { name: CHEAP.into() }
                    })
                    .map(|h| h.id)
            });
            let what = HarnessModel {
                harness: harness.expect("the cheap provider"),
                model: ModelName("fake-a".into()),
            };
            let change = SettingChange::Allow {
                what,
                allowed: false,
            };
            rig.client
                .change_setting(rig.project, change)
                .await
                .unwrap();
        })
    }
    let (rig, _, new) = planned(
        "DISALLOW",
        HELD,
        PlannedReason::Disallowed,
        Some(disallow as fn(&Rig) -> Change<'_>),
    )
    .await;
    assert_eq!(rig.provider(new.driver.id), DEAR);
    rig.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_below_its_agents_narrowed_range_hands_off() {
    fn narrow(rig: &Rig) -> Change<'_> {
        Box::pin(async move {
            let range = EffortRange::new(Effort::Max, Effort::Max).unwrap();
            rig.client
                .set_effort_range(rig.project, rig.ceo, range)
                .await
                .unwrap();
        })
    }
    let (rig, _, new) = planned(
        "NARROW",
        HELD,
        PlannedReason::RangeNarrowed,
        Some(narrow as fn(&Rig) -> Change<'_>),
    )
    .await;
    assert_eq!(new.driver.effort, Effort::Max);
    rig.finish().await;
}
