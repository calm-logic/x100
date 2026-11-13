#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use x100_core::action::AskMeAbout;
use x100_core::effort::EffortRange;
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, JobId};
use x100_core::job::{DaemonJob, Interval, ProjectJob, Schedule};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::Settings;
use x100_core::time::{Clock, FakeClock, Timestamp};
use x100_jobs::{Fired, Scheduler};
use x100_store::{DaemonLog, ProjectLog, open_daemon_log};

pub const START: u64 = 1_000_000;

pub fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

pub fn at(n: u64) -> Timestamp {
    Timestamp::from_millis(n)
}

pub fn every(n: u64) -> Interval {
    Interval::new(ms(n)).unwrap()
}

pub fn ceo() -> AgentId {
    AgentId::from_u128(1)
}

/// A job that wakes the CEO; the note tells jobs apart.
pub fn wake(note: &str) -> ProjectJob {
    ProjectJob::Wake {
        scheduled_by: ceo(),
        agent: ceo(),
        note: note.into(),
    }
}

pub fn note(job: &ProjectJob) -> &str {
    match job {
        ProjectJob::Wake { note, .. } => note,
        other => panic!("not a wake: {other:?}"),
    }
}

/// No minimum, for tests that are not about it.
pub fn no_minimum() -> Interval {
    every(1)
}

pub fn once(at_ms: u64) -> Schedule {
    Schedule::Once { at: at(at_ms) }
}

pub fn repeat(first: u64, every_ms: u64) -> Schedule {
    Schedule::Every {
        first: at(first),
        every: every(every_ms),
    }
}

/// A project log in `dir`, created if new, with the scheduler over it. Drop both to stop the
/// daemon; open again to restart it.
pub fn open_project(
    dir: &Path,
    fake: &FakeClock,
) -> (ProjectLog, Scheduler<x100_core::fold::ProjectState>) {
    open_project_on(dir, Arc::new(fake.clone()))
}

pub fn open_project_on(
    dir: &Path,
    clock: Arc<dyn Clock>,
) -> (ProjectLog, Scheduler<x100_core::fold::ProjectState>) {
    let log = ProjectLog::open(&dir.join("project.sqlite"), clock.clone()).unwrap();
    if log.last().is_none() {
        log.append(
            Actor::Builder,
            ProjectEvent::Created {
                ceo: ceo(),
                charter: "Test.".into(),
                effort: EffortRange::FULL,
                settings: Settings {
                    ask_me_about: AskMeAbout::HighRiskOnly,
                    budget: Money::from_dollars(1),
                    lab_share: Percent::new(10).unwrap(),
                    trial_before_commit: false,
                    allowed: Default::default(),
                },
            },
        )
        .unwrap();
    }
    let scheduler = Scheduler::new(log.clone(), clock);
    (log, scheduler)
}

pub fn open_daemon(
    dir: &Path,
    fake: &FakeClock,
) -> (DaemonLog, Scheduler<x100_core::fold::DaemonState>) {
    let clock: Arc<dyn Clock> = Arc::new(fake.clone());
    let log = open_daemon_log(dir, clock.clone()).unwrap();
    let scheduler = Scheduler::new(log.clone(), clock);
    (log, scheduler)
}

pub fn fired<S: x100_jobs::JobHost>(s: &Scheduler<S>) -> Vec<Fired<S::Does>> {
    let mut out = Vec::new();
    s.fire_due(|f| out.push(f)).unwrap();
    out
}

pub fn fired_ids<S: x100_jobs::JobHost>(s: &Scheduler<S>) -> Vec<JobId> {
    fired(s).into_iter().map(|f| f.job.id).collect()
}

pub fn cool_off_end() -> DaemonJob {
    use x100_core::harness::{HarnessModel, ModelName};
    use x100_core::id::HarnessId;
    DaemonJob::EndCoolOff(HarnessModel {
        harness: HarnessId::from_u128(1),
        model: ModelName("m".into()),
    })
}
