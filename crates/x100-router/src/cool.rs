//! Cool-offs. A harness and model that fail in a way that looks like the provider's are skipped by
//! every project for the daemon's set time, longer if the provider asked for longer. The end is a
//! job in the daemon's log, so it survives a restart and runs on the fake clock in tests.

use x100_core::event::DaemonEvent;
use x100_core::fold::DaemonState;
use x100_core::harness::{CoolOff, HarnessModel, ProviderFailure};
use x100_core::id::JobId;
use x100_core::job::{DaemonJob, Job, JobEvent, JobState, Schedule};
use x100_core::party::Actor;
use x100_core::time::Timestamp;

/// The daemon's events that start a cool-off of `of` at `now`, ended by the job `ends`. A
/// failure while it already cools off starts it again, and the old end is called off.
pub fn cool_off(
    daemon: &DaemonState,
    of: HarnessModel,
    why: ProviderFailure,
    now: Timestamp,
    ends: JobId,
) -> Vec<(Actor, DaemonEvent)> {
    let mut lasts = daemon.settings().cool_off;
    if let ProviderFailure::RateLimited {
        retry_after: Some(after),
    } = &why
    {
        lasts = lasts.max(*after);
    }
    let until = now + lasts;
    let mut out = Vec::new();
    if let Some(old) = daemon.cooling_off(&of) {
        let scheduled = daemon
            .jobs()
            .any(|j| j.job.id == old.ends && matches!(j.state, JobState::Scheduled { .. }));
        if scheduled {
            out.push(DaemonEvent::Job {
                job: old.ends,
                event: JobEvent::Cancelled { by: Actor::Daemon },
            });
        }
    }
    out.push(DaemonEvent::CoolOffStarted {
        cool_off: CoolOff {
            of: of.clone(),
            why,
            until,
        },
        ends,
    });
    out.push(DaemonEvent::JobScheduled(Job {
        id: ends,
        when: Schedule::Once { at: until },
        does: DaemonJob::EndCoolOff(of),
    }));
    out.into_iter().map(|e| (Actor::Daemon, e)).collect()
}

/// The event that ends a cool-off when its job fires, if that job is still the one that ends it.
pub fn cool_off_over(daemon: &DaemonState, job: &Job<DaemonJob>) -> Option<DaemonEvent> {
    let DaemonJob::EndCoolOff(of) = &job.does;
    daemon
        .cooling_off(of)
        .filter(|c| c.ends == job.id)
        .map(|_| DaemonEvent::CoolOffEnded(of.clone()))
}
