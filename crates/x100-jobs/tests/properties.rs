//! Random schedules, cancels, clock moves and restarts against a real store, checked after every
//! step against a plain model of what each job should have done.

mod common;

use std::collections::BTreeMap;

use common::*;
use proptest::prelude::*;
use x100_core::fold::ProjectState;
use x100_core::id::JobId;
use x100_core::job::JobState;
use x100_core::party::Actor;
use x100_core::time::{Clock, FakeClock};
use x100_jobs::Error;

#[derive(Debug, Clone)]
enum Step {
    Once {
        offset: i64,
    },
    Every {
        offset: i64,
        every: u64,
    },
    Cancel(usize),
    Advance(u64),
    /// Exactly to the earliest due time, where "due" is easiest to get wrong by one.
    AdvanceToNext,
    Run,
    Restart,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => prop_oneof![Just(0i64), -500i64..5_000].prop_map(|offset| Step::Once { offset }),
        2 => (-500i64..5_000, 1u64..3_000).prop_map(|(offset, every)| Step::Every { offset, every }),
        2 => any::<usize>().prop_map(Step::Cancel),
        // Small moves, and jumps across many intervals.
        3 => prop_oneof![0u64..2_000, 0u64..200_000].prop_map(Step::Advance),
        2 => Just(Step::AdvanceToNext),
        3 => Just(Step::Run),
        1 => Just(Step::Restart),
    ]
}

/// What the test expects of one job, worked out without the crate.
struct Expect {
    id: JobId,
    every: Option<u64>,
    due: u64,
    alive: bool,
    fired: u32,
}

impl Expect {
    fn fire_at(&mut self, now: u64) {
        match self.every {
            None => self.alive = false,
            Some(step) => self.due += ((now - self.due) / step + 1) * step,
        }
        self.fired += 1;
    }
}

fn check_all(
    log: &x100_store::ProjectLog,
    jobs: &[Expect],
    cancelled: &[JobId],
    now: u64,
) -> Result<(), TestCaseError> {
    // The log still folds, to the state the scheduler works from.
    let events = log.read_after(None, usize::MAX >> 1).unwrap();
    prop_assert_eq!(ProjectState::fold(&events).unwrap(), log.state());

    let state = log.state();
    for job in jobs {
        let record = state.job(job.id).unwrap();
        prop_assert_eq!(record.fired, job.fired, "fire count of {}", job.id);
        if job.every.is_none() {
            prop_assert!(record.fired <= 1, "a one-off job fired twice");
        }
        if let Some(last) = record.last_fired {
            prop_assert!(last.as_millis() <= now);
        }
        if cancelled.contains(&job.id) {
            prop_assert_eq!(record.state, JobState::Cancelled);
        } else if job.alive {
            match record.state {
                JobState::Scheduled { due, .. } => prop_assert_eq!(due.as_millis(), job.due),
                other => prop_assert!(false, "{other:?} should be scheduled"),
            }
        } else {
            prop_assert_eq!(record.state, JobState::Done);
        }
    }
    Ok(())
}

fn run(steps: Vec<Step>) -> Result<(), TestCaseError> {
    let dir = tempfile::tempdir().unwrap();
    let fake = FakeClock::new(at(START));
    let (mut log, mut sched) = open_project(dir.path(), &fake);
    let mut jobs: Vec<Expect> = Vec::new();
    let mut cancelled: Vec<JobId> = Vec::new();

    for step in steps {
        let now = fake.now().as_millis();
        match step {
            Step::Once { offset } | Step::Every { offset, .. } => {
                let first = now.saturating_add_signed(offset);
                let (when, every) = match step {
                    Step::Every { every, .. } => (repeat(first, every), Some(every)),
                    _ => (once(first), None),
                };
                let id = sched
                    .schedule(Actor::Builder, when, wake("p"), no_minimum())
                    .unwrap();
                jobs.push(Expect {
                    id,
                    every,
                    due: first,
                    alive: true,
                    fired: 0,
                });
            }
            Step::Cancel(pick) => {
                if jobs.is_empty() {
                    continue;
                }
                let index = pick % jobs.len();
                let job = &mut jobs[index];
                let result = sched.cancel(Actor::Builder, job.id);
                if job.alive {
                    prop_assert!(result.is_ok());
                    job.alive = false;
                    cancelled.push(job.id);
                } else {
                    // Ended jobs, cancelled or done, cannot be cancelled.
                    prop_assert!(matches!(
                        &result,
                        Err(Error::Store(e)) if matches!(*e, x100_store::Error::Rejected(_))
                    ));
                }
            }
            Step::Advance(by) => fake.advance(ms(by)),
            Step::AdvanceToNext => {
                let next = jobs.iter().filter(|j| j.alive).map(|j| j.due).min();
                if let Some(next) = next.filter(|n| *n > now) {
                    fake.advance(ms(next - now));
                }
            }
            Step::Run => {
                let got = fired(&sched);
                // Everything due fires exactly once, at the time of the run, in due order, and
                // nothing else does.
                let mut want: Vec<_> = jobs
                    .iter_mut()
                    .filter(|j| j.alive && j.due <= now)
                    .map(|j| (j.due, j.id))
                    .collect();
                want.sort();
                let seen: Vec<_> = got.iter().map(|f| (f.due.as_millis(), f.job.id)).collect();
                prop_assert_eq!(seen, want);
                prop_assert!(got.iter().all(|f| f.at.as_millis() == now));
                for f in &got {
                    jobs.iter_mut()
                        .find(|j| j.id == f.job.id)
                        .unwrap()
                        .fire_at(now);
                }
                prop_assert!(fired(&sched).is_empty(), "a second run fired more");
                let next: BTreeMap<_, _> = jobs
                    .iter()
                    .filter(|j| j.alive)
                    .map(|j| (j.due, ()))
                    .collect();
                prop_assert_eq!(
                    sched.next_due().map(|t| t.as_millis()),
                    next.keys().next().copied()
                );
                prop_assert!(next.keys().all(|d| *d > now), "something due has not fired");
            }
            Step::Restart => {
                drop((log, sched));
                (log, sched) = open_project(dir.path(), &fake);
            }
        }
        check_all(&log, &jobs, &cancelled, fake.now().as_millis())?;
    }
    // Whatever is left fires on a final run, however far behind.
    fake.advance(ms(1_000_000));
    let _ = fired(&sched);
    for j in jobs.iter_mut().filter(|j| j.alive) {
        j.fire_at(fake.now().as_millis());
    }
    check_all(&log, &jobs, &cancelled, fake.now().as_millis())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn the_job_invariants_hold_over_random_schedules_cancels_restarts_and_clock_moves(
        steps in prop::collection::vec(step(), 1..40)
    ) {
        run(steps)?;
    }
}

#[test]
fn the_model_agrees_with_core_on_the_series() {
    // A guard on the test's own arithmetic: due 100, every 50, fired at 275 is next due at 300.
    let mut e = Expect {
        id: JobId::from_u128(1),
        every: Some(50),
        due: 100,
        alive: true,
        fired: 0,
    };
    e.fire_at(275);
    assert_eq!(e.due, 300);
}
