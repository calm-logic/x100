//! Scheduled jobs. One clock fires every job, so everything time-based in x100 takes one path: in
//! the log, surviving a restart, and on a fake clock in tests.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::effort::Effort;
use crate::harness::HarnessModel;
use crate::id::{AgentId, DriverId, JobId, WaitId};
use crate::machine::{MachineKind, Rejected};
use crate::party::Actor;
use crate::time::Timestamp;

/// A job as it is scheduled. `D` is what it does: [`ProjectJob`] in a project's log, [`DaemonJob`]
/// in the daemon's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Job<D> {
    pub id: JobId,
    pub when: Schedule,
    pub does: D,
}

/// When a job fires. A delay is turned into a time when the job is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Schedule {
    Once { at: Timestamp },
    Every { first: Timestamp, every: Interval },
}

/// The time between firings of a repeating job. Never zero. The minimum a builder or agent may ask
/// for is a setting checked when the job is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Duration", into = "Duration")]
pub struct Interval(Duration);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a repeat must be at least a millisecond")]
pub struct ZeroInterval;

impl Interval {
    pub fn new(every: Duration) -> Result<Self, ZeroInterval> {
        if every.as_millis() > 0 {
            Ok(Self(every))
        } else {
            Err(ZeroInterval)
        }
    }

    pub fn get(self) -> Duration {
        self.0
    }
}

impl TryFrom<Duration> for Interval {
    type Error = ZeroInterval;

    fn try_from(d: Duration) -> Result<Self, ZeroInterval> {
        Interval::new(d)
    }
}

impl From<Interval> for Duration {
    fn from(i: Interval) -> Duration {
        i.0
    }
}

/// What a job in a project does. Each variant names who scheduled it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectJob {
    /// Deliver a note to an agent's inbox: itself or one of its children.
    Wake {
        scheduled_by: AgentId,
        agent: AgentId,
        note: String,
    },
    /// Delegate a fresh task from the same description at every firing, as if `scheduled_by`
    /// delegated it.
    Delegate {
        scheduled_by: AgentId,
        to: AgentId,
        title: String,
        ask: String,
        effort: Effort,
    },
    /// One of the daemon's own routines.
    Routine(ProjectRoutine),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRoutine {
    /// A warm driver's warm period is over and it retires.
    EndWarmPeriod { driver: DriverId },
    /// A wait reached its deadline.
    WaitDeadline { agent: AgentId, wait: WaitId },
}

/// What a job in the daemon's log does. These belong to no project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonJob {
    EndCoolOff(HarnessModel),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Scheduled {
        due: Timestamp,
        every: Option<Interval>,
    },
    Done,
    Cancelled,
}

impl JobState {
    pub fn initial(when: &Schedule) -> JobState {
        match *when {
            Schedule::Once { at } => JobState::Scheduled {
                due: at,
                every: None,
            },
            Schedule::Every { first, every } => JobState::Scheduled {
                due: first,
                every: Some(every),
            },
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            JobState::Scheduled { .. } => "Scheduled",
            JobState::Done => "Done",
            JobState::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobEvent {
    Fired { at: Timestamp },
    Cancelled { by: Actor },
}

impl JobEvent {
    pub fn name(&self) -> &'static str {
        match self {
            JobEvent::Fired { .. } => "Fired",
            JobEvent::Cancelled { .. } => "Cancelled",
        }
    }
}

/// The job machine. Scheduled → Done when a one-off job fires, or back to Scheduled with its next
/// time when a repeating job fires. Scheduled → Cancelled. A job never fires before it is due.
///
/// A repeating job's next time is the first one in its series after the firing, so a job that fell
/// due many times while the daemon was stopped fires once when it starts.
pub fn next(state: &JobState, event: &JobEvent) -> Result<JobState, Rejected> {
    let rejected = |event: &'static str| Rejected {
        machine: MachineKind::Job,
        state: state.name(),
        event,
    };
    match (*state, *event) {
        (JobState::Scheduled { due, .. }, JobEvent::Fired { at }) if at < due => {
            Err(rejected("Fired before due"))
        }
        (JobState::Scheduled { every: None, .. }, JobEvent::Fired { .. }) => Ok(JobState::Done),
        (
            JobState::Scheduled {
                due,
                every: Some(every),
            },
            JobEvent::Fired { at },
        ) => Ok(JobState::Scheduled {
            due: next_after(due, every, at),
            every: Some(every),
        }),
        (JobState::Scheduled { .. }, JobEvent::Cancelled { .. }) => Ok(JobState::Cancelled),
        _ => Err(rejected(event.name())),
    }
}

/// The first time in the series `due + k * every` that is after `at`.
fn next_after(due: Timestamp, every: Interval, at: Timestamp) -> Timestamp {
    let step = u64::try_from(every.get().as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    let behind = at.as_millis().saturating_sub(due.as_millis());
    let steps = behind / step + 1;
    Timestamp::from_millis(due.as_millis().saturating_add(steps.saturating_mul(step)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn a_repeating_job_is_next_due_at_the_first_time_in_its_series_after_it_fired(
            due in 0u64..1_000_000, every in 1u64..10_000, late in 0u64..1_000_000
        ) {
            let interval = Interval::new(Duration::from_millis(every)).unwrap();
            let at = Timestamp::from_millis(due + late);
            let due_at = Timestamp::from_millis(due);
            let state = JobState::Scheduled { due: due_at, every: Some(interval) };
            let after = next(&state, &JobEvent::Fired { at }).unwrap();
            let JobState::Scheduled { due: next_due, .. } = after else {
                panic!("a repeating job stays scheduled");
            };
            let next_due = next_due.as_millis();
            prop_assert!(next_due > at.as_millis());
            prop_assert!(next_due - at.as_millis() <= every);
            prop_assert_eq!((next_due - due) % every, 0);
        }
    }

    #[test]
    fn a_zero_repeat_cannot_be_made() {
        assert_eq!(Interval::new(Duration::ZERO), Err(ZeroInterval));
        assert!(serde_json::from_str::<Interval>(r#"{"secs":0,"nanos":0}"#).is_err());
    }
}
