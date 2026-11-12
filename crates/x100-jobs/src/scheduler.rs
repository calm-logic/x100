use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;
use ulid::Ulid;
use x100_core::id::JobId;
use x100_core::job::{Interval, Job, JobEvent, Schedule};
use x100_core::party::Actor;
use x100_core::time::{Clock, Timestamp};
use x100_store::EventLog;

use crate::error::Error;
use crate::host::JobHost;
use crate::step;

/// The longest the runner sleeps before it looks at the clock again. A monotonic timer does not
/// count time the machine spent suspended, so a job due meanwhile would fire late by the rest of a
/// long sleep.
const LONGEST_SLEEP: Duration = Duration::from_secs(30);

/// A job that has fired: its `Fired` event is in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired<D> {
    pub job: Job<D>,
    /// The time it was due, which is before `at` when the daemon was late or was stopped.
    pub due: Timestamp,
    pub at: Timestamp,
}

/// Schedules, cancels and fires the jobs of one log. Cloning shares it, and so may several
/// schedulers over clones of one log: the append decides who fired a job, so it fires once.
///
/// The methods block on SQLite like the store's. Async code calls them through `spawn_blocking`;
/// [`Scheduler::run`] does.
pub struct Scheduler<S: JobHost> {
    log: EventLog<S>,
    clock: Arc<dyn Clock>,
    // Woken when a job is scheduled or cancelled, so the runner's sleep follows the earliest job.
    changed: Arc<Notify>,
}

impl<S: JobHost> Clone for Scheduler<S> {
    fn clone(&self) -> Self {
        Self {
            log: self.log.clone(),
            clock: self.clock.clone(),
            changed: self.changed.clone(),
        }
    }
}

impl<S: JobHost> Scheduler<S> {
    /// A scheduler over `log` reading time from `clock`, which should be the clock the log was
    /// opened with. Everything it needs is in the log's folded state, so after a restart this is
    /// all it takes to carry on.
    pub fn new(log: EventLog<S>, clock: Arc<dyn Clock>) -> Self {
        Self {
            log,
            clock,
            changed: Arc::new(Notify::new()),
        }
    }

    /// Schedules a job. A repeat shorter than `minimum_repeat` (the daemon's setting, which the
    /// caller reads) is refused with [`Error::RepeatTooShort`] and nothing is written. Core
    /// exempts no job from it, the daemon's own included.
    pub fn schedule(
        &self,
        by: Actor,
        when: Schedule,
        does: S::Does,
        minimum_repeat: Interval,
    ) -> Result<JobId, Error> {
        if let Schedule::Every { every, .. } = when
            && every.get() < minimum_repeat.get()
        {
            return Err(Error::RepeatTooShort {
                asked: every,
                minimum: minimum_repeat,
            });
        }
        let now = self.clock.now();
        let id = JobId::new(Ulid::from_parts(now.as_millis(), Ulid::generate().random()));
        self.log.append(by, S::scheduled(Job { id, when, does }))?;
        self.changed.notify_one();
        Ok(id)
    }

    /// Schedules a one-off job `delay` from now.
    pub fn schedule_after(
        &self,
        by: Actor,
        delay: Duration,
        does: S::Does,
        minimum_repeat: Interval,
    ) -> Result<JobId, Error> {
        let at = self.clock.now() + delay;
        self.schedule(by, Schedule::Once { at }, does, minimum_repeat)
    }

    /// Cancels a job, which never fires afterwards.
    pub fn cancel(&self, by: Actor, job: JobId) -> Result<(), Error> {
        self.log
            .append(by, S::job_event(job, JobEvent::Cancelled { by }))?;
        self.changed.notify_one();
        Ok(())
    }

    /// Tells the runner the jobs changed, for a job appended to the log other than through
    /// [`Scheduler::schedule`], such as in one batch with other events. Without it the runner
    /// notices such a job only at its next look at the clock, which may be [`LONGEST_SLEEP`] away.
    pub fn wake(&self) {
        self.changed.notify_one();
    }

    /// When the next job falls due, or `None` with nothing scheduled. May be in the past.
    pub fn next_due(&self) -> Option<Timestamp> {
        self.log.with_state(|s| step::next_due(states(s)))
    }

    /// Fires every job due now, earliest first, handing each to `hand_over` as it is fired.
    ///
    /// A job is fired by appending its `Fired` event, and `hand_over` runs only after that
    /// append succeeded. If the process dies before the append, the job is still due and fires
    /// on the next start; if it dies after, the job has fired and is not handed over again. A
    /// repeating job fires once however many intervals have passed, and its next time is the
    /// next one in its series. An error stops the pass; what was fired before it was handed over.
    pub fn fire_due(&self, mut hand_over: impl FnMut(Fired<S::Does>)) -> Result<(), Error> {
        let now = self.clock.now();
        for due in self.log.with_state(|s| step::due(states(s), now)) {
            let event = S::job_event(due.job, JobEvent::Fired { at: now });
            match self.log.append(Actor::Daemon, event) {
                Ok(_) => {}
                // Cancelled, or fired by another scheduler, since the list was made.
                Err(x100_store::Error::Rejected(_)) => continue,
                Err(e) => return Err(e.into()),
            }
            let job = self.log.with_state(|s| {
                s.records()
                    .find(|r| r.job.id == due.job)
                    .map(|r| r.job.clone())
                    .expect("a job just fired is in the log")
            });
            hand_over(Fired {
                job,
                due: due.due,
                at: now,
            });
        }
        Ok(())
    }

    /// Fires jobs as they fall due and sends each to `out`, until `out` is closed or the log
    /// fails. For the real clock: it sleeps until the next job, and wakes early when a job is
    /// scheduled or cancelled. On a fake clock it would sleep in real time, so tests call
    /// [`Scheduler::fire_due`] instead.
    pub async fn run(self, out: UnboundedSender<Fired<S::Does>>) -> Result<(), Error> {
        loop {
            let pass = self.clone();
            let sink = out.clone();
            let closed = tokio::task::spawn_blocking(move || {
                let mut closed = false;
                pass.fire_due(|fired| closed |= sink.send(fired).is_err())?;
                Ok::<_, Error>(closed)
            })
            .await
            .expect("firing jobs panicked")?;
            if closed {
                return Ok(());
            }
            // A change made after this point leaves a permit in `changed`, so the wait ends at
            // once and nothing is slept through.
            let wait = self.next_due().map_or(LONGEST_SLEEP, |due| {
                due.since(self.clock.now()).min(LONGEST_SLEEP)
            });
            tokio::select! {
                () = self.changed.notified() => {}
                () = tokio::time::sleep(wait) => {}
                () = out.closed() => return Ok(()),
            }
        }
    }
}

fn states<S: JobHost>(s: &S) -> impl Iterator<Item = (JobId, x100_core::job::JobState)> + '_ {
    s.records().map(|r| (r.job.id, r.state))
}
