//! "No agent waits forever: every wait has a deadline, and a wait on something that can no longer
//! happen ends."
//!
//! An agent waits on a job, with a deadline that is itself a job. Time passes, and the scheduler
//! fires or cancels either job. Each job event goes through the real job machine, and the real
//! `Wait::ended_by` decides whether it ends the wait. When a wait ends, its deadline job is
//! cancelled.

use stateright::{Model, Property};

use super::{is_terminal, prove, refute};
use crate::event::ProjectEvent;
use crate::id::{JobId, WaitId};
use crate::job::{self, JobEvent, JobState, Schedule};
use crate::party::Actor;
use crate::time::Timestamp;
use crate::wait::{Wait, WaitEnd, WaitOn};

type EndedBy = fn(&Wait, &ProjectEvent) -> Option<WaitEnd>;

const TARGET: JobId = JobId::from_u128(1);
const DEADLINE: JobId = JobId::from_u128(2);
const END_OF_TIME: u64 = 6;

struct Waiting {
    ended_by: EndedBy,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct State {
    now: u64,
    target: JobState,
    deadline: JobState,
    open: bool,
    end: Option<WaitEnd>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Tick,
    Fire(JobId),
    Cancel(JobId),
}

fn wait() -> Wait {
    Wait {
        id: WaitId::from_u128(1),
        on: WaitOn::Job(TARGET),
        deadline: Timestamp::from_millis(4),
        deadline_job: DEADLINE,
    }
}

impl Model for Waiting {
    type State = State;
    type Action = Step;

    fn init_states(&self) -> Vec<State> {
        // The awaited job is due before the deadline, or after it.
        [2, 5]
            .into_iter()
            .map(|due| State {
                now: 0,
                target: JobState::initial(&Schedule::Once {
                    at: Timestamp::from_millis(due),
                }),
                deadline: JobState::initial(&Schedule::Once {
                    at: wait().deadline,
                }),
                open: true,
                end: None,
            })
            .collect()
    }

    fn actions(&self, s: &State, actions: &mut Vec<Step>) {
        if s.now < END_OF_TIME {
            actions.push(Step::Tick);
        }
        for job in [TARGET, DEADLINE] {
            actions.push(Step::Fire(job));
        }
        // Anyone may cancel the awaited job. Only the daemon cancels a deadline, when its wait
        // ends.
        actions.push(Step::Cancel(TARGET));
    }

    fn next_state(&self, s: &State, step: Step) -> Option<State> {
        let mut s = s.clone();
        let (job, event) = match step {
            Step::Tick => {
                s.now += 1;
                return Some(s);
            }
            Step::Fire(job) => (
                job,
                JobEvent::Fired {
                    at: Timestamp::from_millis(s.now),
                },
            ),
            Step::Cancel(job) => (job, JobEvent::Cancelled { by: Actor::Builder }),
        };
        let slot = if job == TARGET {
            &mut s.target
        } else {
            &mut s.deadline
        };
        *slot = job::next(slot, &event).ok()?;
        if s.open
            && let Some(end) = (self.ended_by)(&wait(), &ProjectEvent::Job { job, event })
        {
            s.open = false;
            s.end = Some(end);
            if let Ok(cancelled) =
                job::next(&s.deadline, &JobEvent::Cancelled { by: Actor::Daemon })
            {
                s.deadline = cancelled;
            }
        }
        Some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always(
                "every open wait has a deadline still to fire",
                |_, s: &State| !s.open || matches!(s.deadline, JobState::Scheduled { .. }),
            ),
            Property::always(
                "a wait on something that can no longer happen ends",
                |_, s: &State| !s.open || matches!(s.target, JobState::Scheduled { .. }),
            ),
            Property::always(
                "no wait is open when nothing more can happen",
                |m, s: &State| !s.open || !is_terminal(m, s),
            ),
            Property::sometimes("a wait is satisfied", |_, s: &State| {
                s.end == Some(WaitEnd::Satisfied)
            }),
            Property::sometimes("a wait expires", |_, s: &State| {
                s.end == Some(WaitEnd::Expired)
            }),
            Property::sometimes("a wait cannot happen", |_, s: &State| {
                s.end == Some(WaitEnd::CannotHappen)
            }),
        ]
    }
}

/// Bug: a cancelled job is not seen as ending a wait on it.
fn ignores_cancelled_job(wait: &Wait, event: &ProjectEvent) -> Option<WaitEnd> {
    match event {
        ProjectEvent::Job {
            event: JobEvent::Cancelled { .. },
            ..
        } => None,
        _ => wait.ended_by(event),
    }
}

#[test]
fn no_agent_waits_forever() {
    prove(
        "wait",
        Waiting {
            ended_by: Wait::ended_by,
        },
    );
}

#[test]
fn a_wait_left_open_on_a_cancelled_job_is_caught() {
    refute(
        "wait with bug ignores_cancelled_job",
        Waiting {
            ended_by: ignores_cancelled_job,
        },
        "a wait on something that can no longer happen ends",
    );
}
