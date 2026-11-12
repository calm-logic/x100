//! Every transition of the six state machines, and the rejection of every event a state does not
//! accept.

mod common;

use std::time::Duration;

use common::check_table;
use x100_core::MachineKind;
use x100_core::action::{
    self, ActionEvent, ActionKind, ActionState, Classification, ClassifiedBy, Risk,
};
use x100_core::agent::{self, AgentEvent, AgentState, Waiting};
use x100_core::driver::{
    self, ContextFill, DriverEvent, DriverState, EndReason, Failure, Incompatible, PlannedReason,
    RetireReason, StopReason,
};
use x100_core::effort::{Effort, EffortRange};
use x100_core::git::CommitId;
use x100_core::id::{AgentId, InboxItemId, JobId, TaskId, WaitId};
use x100_core::inbox::InboxItem;
use x100_core::job::{self, Interval, JobEvent, JobState};
use x100_core::money::Money;
use x100_core::party::{Actor, Party};
use x100_core::task::{self, Assignment, Outcome, Progress, Report, TaskEvent, TaskState, Verdict};
use x100_core::time::Timestamp;
use x100_core::trial::{self, Arm, RunScore, TrialEvent, TrialState, Verdict as TrialVerdict};
use x100_core::wait::{Wait, WaitEnd, WaitOn};

#[test]
fn driver_transitions() {
    use DriverEvent as E;
    use DriverState as S;
    let ended = S::Ended(EndReason::Retired(RetireReason::WarmTooLong));
    let failed = S::Failed(Failure::StreamBroke);
    let handing_off = S::HandingOff(PlannedReason::ContextFull);
    let states = [
        S::Starting,
        S::Running,
        S::Warm,
        S::Compacting,
        handing_off.clone(),
        ended.clone(),
        failed.clone(),
    ];

    let turn = E::TurnStarted {
        given: vec![InboxItemId::from_u128(1)],
    };
    let fill = E::ContextReported(ContextFill {
        used: 10,
        window: 100,
    });
    let stop = E::Stopped(StopReason::ReplacedByBuilder);
    let fail = E::Failed(Failure::StreamBroke);
    let events = [
        turn.clone(),
        E::TurnEnded,
        fill.clone(),
        E::CompactionStarted,
        E::Compacted,
        E::HandoffStarted(PlannedReason::ContextFull),
        E::Retired(RetireReason::WarmTooLong),
        stop.clone(),
        fail.clone(),
    ];

    let stopped = S::Ended(EndReason::Stopped(StopReason::ReplacedByBuilder));
    let mut accepted = vec![
        (S::Starting, turn.clone(), S::Running),
        (S::Warm, turn, S::Running),
        (S::Running, E::TurnEnded, S::Warm),
        (
            handing_off.clone(),
            E::TurnEnded,
            S::Ended(EndReason::HandedOff(PlannedReason::ContextFull)),
        ),
        (S::Warm, E::CompactionStarted, S::Compacting),
        (S::Compacting, E::Compacted, S::Warm),
        (
            S::Warm,
            E::HandoffStarted(PlannedReason::ContextFull),
            handing_off.clone(),
        ),
        (S::Warm, E::Retired(RetireReason::WarmTooLong), ended),
    ];
    // Every live state reports its context, can be stopped, and can fail.
    for live in [S::Starting, S::Running, S::Warm, S::Compacting, handing_off] {
        accepted.push((live.clone(), fill.clone(), live.clone()));
        accepted.push((live.clone(), stop.clone(), stopped.clone()));
        accepted.push((live, fail.clone(), failed.clone()));
    }
    check_table(
        MachineKind::Driver,
        driver::next,
        DriverState::name,
        &states,
        &events,
        &accepted,
    );
}

#[test]
fn only_final_driver_states_are_not_live() {
    assert!(DriverState::Warm.is_live());
    assert!(DriverState::HandingOff(PlannedReason::EffortChanged).is_live());
    assert!(!DriverState::Failed(Failure::LastTurnFailed).is_live());
    assert!(!DriverState::Ended(EndReason::Stopped(StopReason::DaemonRestarted)).is_live());
    assert!(!Failure::DaemonRestarted.counts_against_harness());
    assert!(
        Failure::Stalled {
            silent_for: Duration::from_secs(60)
        }
        .counts_against_harness()
    );
}

#[test]
fn agent_transitions() {
    use AgentEvent as E;
    use AgentState as S;
    let for_driver = S::Waiting(Waiting::ForDriver(vec![Incompatible::NoEffortInRange]));
    let on_waits = S::Waiting(Waiting::OnWaits);
    let states = [
        S::Planned,
        S::Working,
        on_waits.clone(),
        for_driver.clone(),
        S::Idle,
        S::Retired,
    ];

    let delivered = E::Delivered {
        item: InboxItemId::from_u128(1),
        what: InboxItem::Task {
            task: TaskId::from_u128(1),
        },
    };
    let wait = Wait {
        id: WaitId::from_u128(1),
        on: WaitOn::Report(TaskId::from_u128(2)),
        deadline: Timestamp::from_millis(100),
        deadline_job: JobId::from_u128(3),
    };
    let wait_ended = E::WaitEnded {
        wait: wait.id,
        end: WaitEnd::Expired,
    };
    let no_driver = E::NoDriver(vec![Incompatible::NothingAllowed]);
    let range = EffortRange::new(Effort::Low, Effort::High).unwrap();
    let team_changes = [
        E::EffortSet(range),
        E::BudgetSet(Money::from_dollars(5)),
        E::Moved {
            under: AgentId::from_u128(9),
        },
        E::Promoted {
            effort: range,
            budget: Money::from_dollars(5),
        },
    ];
    let mut events = vec![
        delivered.clone(),
        E::WaitOpened(wait),
        wait_ended.clone(),
        no_driver.clone(),
        E::DriverFound,
        E::WentIdle,
        E::Retired,
    ];
    events.extend(team_changes.iter().cloned());

    let mut accepted = vec![
        (S::Working, events[1].clone(), on_waits.clone()),
        (
            S::Working,
            no_driver,
            S::Waiting(Waiting::ForDriver(vec![Incompatible::NothingAllowed])),
        ),
        (for_driver.clone(), E::DriverFound, S::Working),
        (S::Working, E::WentIdle, S::Idle),
        // The wait for a driver reached its deadline and the task was reported failed.
        (for_driver.clone(), E::WentIdle, S::Idle),
        (S::Idle, E::Retired, S::Retired),
        // A planned lead a team change removes before any work reached it.
        (S::Planned, E::Retired, S::Retired),
    ];
    for s in [S::Working, on_waits.clone(), for_driver.clone()] {
        accepted.push((s.clone(), wait_ended.clone(), s));
    }
    // Anything reaching the inbox makes a planned, waiting or idle agent work. Team changes leave
    // the state as it is. A retired agent accepts neither.
    for s in [S::Planned, S::Working, on_waits, for_driver, S::Idle] {
        accepted.push((s.clone(), delivered.clone(), S::Working));
        for change in &team_changes {
            accepted.push((s.clone(), change.clone(), s.clone()));
        }
    }
    check_table(
        MachineKind::Agent,
        agent::next,
        AgentState::name,
        &states,
        &events,
        &accepted,
    );
}

#[test]
fn job_transitions() {
    let every = Interval::new(Duration::from_millis(5)).unwrap();
    let once = JobState::Scheduled {
        due: Timestamp::from_millis(10),
        every: None,
    };
    let repeating = JobState::Scheduled {
        due: Timestamp::from_millis(10),
        every: Some(every),
    };
    let states = [once, repeating, JobState::Done, JobState::Cancelled];

    let fired = |ms| JobEvent::Fired {
        at: Timestamp::from_millis(ms),
    };
    let cancel = JobEvent::Cancelled { by: Actor::Builder };
    let events = [fired(9), fired(10), fired(23), cancel];

    let accepted = [
        (once, fired(10), JobState::Done),
        (once, fired(23), JobState::Done),
        (
            repeating,
            fired(10),
            JobState::Scheduled {
                due: Timestamp::from_millis(15),
                every: Some(every),
            },
        ),
        // Due at 10, 15 and 20 while it was not fired: it fires once, and is next due at 25.
        (
            repeating,
            fired(23),
            JobState::Scheduled {
                due: Timestamp::from_millis(25),
                every: Some(every),
            },
        ),
        (once, cancel, JobState::Cancelled),
        (repeating, cancel, JobState::Cancelled),
    ];
    check_table(
        MachineKind::Job,
        job::next,
        JobState::name,
        &states,
        &events,
        &accepted,
    );
}

#[test]
fn a_job_never_fires_early() {
    let once = JobState::Scheduled {
        due: Timestamp::from_millis(10),
        every: None,
    };
    let err = job::next(
        &once,
        &JobEvent::Fired {
            at: Timestamp::from_millis(9),
        },
    )
    .unwrap_err();
    assert_eq!(err.event, "Fired before due");
}

#[test]
fn action_transitions() {
    use ActionEvent as E;
    use ActionState as S;
    let states = [
        S::Proposed,
        S::Classified,
        S::Allowed,
        S::Held,
        S::Approved,
        S::Denied,
        S::CarriedOut,
        S::Failed,
    ];
    let classified = E::Classified(Classification {
        kind: ActionKind::DeployToProduction,
        risk: Risk::High,
        by: ClassifiedBy::Rules,
        why: "deploys to production".into(),
    });
    let failed = E::Failed {
        error: "exit 1".into(),
    };
    let events = [
        classified.clone(),
        E::Allowed,
        E::Held,
        E::Approved,
        E::Denied,
        E::CarriedOut,
        failed.clone(),
    ];
    let accepted = [
        (S::Proposed, classified, S::Classified),
        (S::Classified, E::Allowed, S::Allowed),
        (S::Classified, E::Held, S::Held),
        (S::Held, E::Approved, S::Approved),
        (S::Held, E::Denied, S::Denied),
        (S::Allowed, E::CarriedOut, S::CarriedOut),
        (S::Approved, E::CarriedOut, S::CarriedOut),
        (S::Allowed, failed.clone(), S::Failed),
        (S::Approved, failed, S::Failed),
    ];
    check_table(
        MachineKind::Action,
        action::next,
        |s| s.name(),
        &states,
        &events,
        &accepted,
    );
}

#[test]
fn task_transitions() {
    use TaskEvent as E;
    use TaskState as S;
    let a = Assignment {
        delegator: Party::Builder,
        assignee: AgentId::from_u128(1),
        effort: Effort::Medium,
    };
    let higher = Assignment {
        effort: Effort::High,
        ..a.clone()
    };
    let done = Report {
        outcome: Outcome::Completed,
        summary: "done".into(),
    };
    let states = [
        S::NewIdea,
        S::InProgress(a.clone()),
        S::InReview(a.clone(), done.clone()),
        S::Promoting(a.clone(), done.clone()),
        S::Done(a.clone(), done.clone()),
    ];
    let progress = E::ProgressNoted(Progress {
        note: "half".into(),
        done: None,
    });
    let send_back = E::Reviewed(Verdict::SendBack {
        note: "tests fail".into(),
        effort: Some(Effort::High),
    });
    let merged = E::Merged {
        commit: CommitId("abc".into()),
    };
    let conflict = E::MergeFailed {
        conflict: "src/lib.rs".into(),
    };
    let events = [
        E::Delegated(a.clone()),
        progress.clone(),
        E::EffortChanged(Effort::High),
        E::Reported(done.clone()),
        E::Reviewed(Verdict::Accept),
        send_back.clone(),
        merged.clone(),
        conflict.clone(),
    ];
    let accepted = [
        (
            S::NewIdea,
            E::Delegated(a.clone()),
            S::InProgress(a.clone()),
        ),
        (S::InProgress(a.clone()), progress, S::InProgress(a.clone())),
        (
            S::InProgress(a.clone()),
            E::EffortChanged(Effort::High),
            S::InProgress(higher),
        ),
        (
            S::InProgress(a.clone()),
            E::Reported(done.clone()),
            S::InReview(a.clone(), done.clone()),
        ),
        (
            S::InReview(a.clone(), done.clone()),
            E::Reviewed(Verdict::Accept),
            S::Promoting(a.clone(), done.clone()),
        ),
        (
            S::InReview(a.clone(), done.clone()),
            send_back,
            S::InProgress(a.clone()),
        ),
        (
            S::Promoting(a.clone(), done.clone()),
            merged,
            S::Done(a.clone(), done.clone()),
        ),
        (S::Promoting(a.clone(), done), conflict, S::InProgress(a)),
    ];
    check_table(
        MachineKind::Task,
        task::next,
        TaskState::name,
        &states,
        &events,
        &accepted,
    );
}

#[test]
fn trial_transitions() {
    use TrialEvent as E;
    use TrialState as S;
    let supports = S::Scored(TrialVerdict::Supports);
    let does_not = S::Scored(TrialVerdict::DoesNotSupport);
    let states = [
        S::Proposed,
        S::Running,
        supports,
        does_not,
        S::Committed,
        S::Discarded,
        S::Stopped,
    ];
    let run = E::RunFinished(RunScore {
        arm: Arm::Changed,
        run: 1,
        checks_passed: 3,
        checks_total: 3,
        tool_errors: 0,
        turns: 4,
        duration: Duration::from_secs(30),
        cost: Money::from_micros(1_000),
        judged: None,
    });
    let commit = E::Committed {
        commit: CommitId("abc".into()),
    };
    let events = [
        E::Started,
        run.clone(),
        E::Scored(TrialVerdict::Supports),
        E::Scored(TrialVerdict::DoesNotSupport),
        commit.clone(),
        E::Discarded,
        E::Stopped,
    ];
    let accepted = [
        (S::Proposed, E::Started, S::Running),
        (S::Running, run, S::Running),
        (S::Running, E::Scored(TrialVerdict::Supports), supports),
        (
            S::Running,
            E::Scored(TrialVerdict::DoesNotSupport),
            does_not,
        ),
        (S::Running, E::Stopped, S::Stopped),
        (supports, commit, S::Committed),
        (does_not, E::Discarded, S::Discarded),
    ];
    check_table(
        MachineKind::Trial,
        trial::next,
        |s| s.name(),
        &states,
        &events,
        &accepted,
    );
}
