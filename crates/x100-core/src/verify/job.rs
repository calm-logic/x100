//! "A cancelled job never fires, and a one-off job fires at most once", whatever the scheduler
//! attempts and whenever, including after the daemon was stopped past several due times.

use std::time::Duration;

use stateright::{Model, Property};

use super::{prove, refute};
use crate::Rejected;
use crate::job::{self, Interval, JobEvent, JobState, Schedule};
use crate::party::Actor;
use crate::time::Timestamp;

type Next = fn(&JobState, &JobEvent) -> Result<JobState, Rejected>;

const END_OF_TIME: u64 = 9;

struct Jobs {
    next: Next,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct State {
    job: JobState,
    one_off: bool,
    now: u64,
    fired: u8,
    cancelled: bool,
    fired_after_cancel: bool,
    fired_early: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// Time passes: a tick, or the daemon stopped across several due times.
    Wait(u64),
    /// The scheduler tries to fire the job now, due or not.
    Fire,
    Cancel,
}

impl Model for Jobs {
    type State = State;
    type Action = Step;

    fn init_states(&self) -> Vec<State> {
        let every = Interval::new(Duration::from_millis(2)).expect("not zero");
        [
            Schedule::Once {
                at: Timestamp::from_millis(2),
            },
            Schedule::Every {
                first: Timestamp::from_millis(2),
                every,
            },
        ]
        .iter()
        .map(|when| State {
            job: JobState::initial(when),
            one_off: matches!(when, Schedule::Once { .. }),
            now: 0,
            fired: 0,
            cancelled: false,
            fired_after_cancel: false,
            fired_early: false,
        })
        .collect()
    }

    fn actions(&self, s: &State, actions: &mut Vec<Step>) {
        if s.now < END_OF_TIME {
            actions.push(Step::Wait(1));
            actions.push(Step::Wait(5));
        }
        actions.push(Step::Fire);
        actions.push(Step::Cancel);
    }

    fn next_state(&self, s: &State, step: Step) -> Option<State> {
        let mut s = s.clone();
        match step {
            Step::Wait(by) => s.now = (s.now + by).min(END_OF_TIME),
            Step::Fire => {
                let due = match s.job {
                    JobState::Scheduled { due, .. } => Some(due.as_millis()),
                    _ => None,
                };
                s.job = (self.next)(
                    &s.job,
                    &JobEvent::Fired {
                        at: Timestamp::from_millis(s.now),
                    },
                )
                .ok()?;
                // Counting past two is enough for every property, and keeps a buggy model finite.
                s.fired = (s.fired + 1).min(2);
                s.fired_after_cancel |= s.cancelled;
                s.fired_early |= due.is_none_or(|due| s.now < due);
            }
            Step::Cancel => {
                s.job = (self.next)(&s.job, &JobEvent::Cancelled { by: Actor::Builder }).ok()?;
                s.cancelled = true;
            }
        }
        Some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("a cancelled job never fires", |_, s: &State| {
                !s.fired_after_cancel
            }),
            Property::always("a one-off job fires at most once", |_, s: &State| {
                !s.one_off || s.fired <= 1
            }),
            Property::always("a job never fires before it is due", |_, s: &State| {
                !s.fired_early
            }),
            Property::sometimes("a repeating job fires again", |_, s: &State| {
                !s.one_off && s.fired >= 2
            }),
            Property::sometimes("a one-off job fires", |_, s: &State| {
                s.one_off && s.fired == 1
            }),
        ]
    }
}

/// Bug: a one-off job is treated as if it repeated, so it is still scheduled after it fires.
fn one_off_stays_scheduled(s: &JobState, e: &JobEvent) -> Result<JobState, Rejected> {
    match (s, e) {
        (JobState::Scheduled { due, every: None }, JobEvent::Fired { at }) if at >= due => Ok(*s),
        _ => job::next(s, e),
    }
}

/// Bug: firing a cancelled job is accepted.
fn cancelled_job_fires(s: &JobState, e: &JobEvent) -> Result<JobState, Rejected> {
    match (s, e) {
        (JobState::Cancelled, JobEvent::Fired { .. }) => Ok(JobState::Cancelled),
        _ => job::next(s, e),
    }
}

#[test]
fn cancelled_jobs_never_fire_and_one_off_jobs_fire_once() {
    prove("job", Jobs { next: job::next });
}

#[test]
fn a_one_off_job_that_stays_scheduled_is_caught() {
    refute(
        "job with bug one_off_stays_scheduled",
        Jobs {
            next: one_off_stays_scheduled,
        },
        "a one-off job fires at most once",
    );
}

#[test]
fn a_cancelled_job_that_fires_is_caught() {
    refute(
        "job with bug cancelled_job_fires",
        Jobs {
            next: cancelled_job_fires,
        },
        "a cancelled job never fires",
    );
}
