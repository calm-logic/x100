//! A cool-off lasts the daemon's set time, or longer if the provider asked, ends by its job, and a
//! second failure restarts it and calls the first end off.

mod common;

use std::time::Duration;

use common::*;
use x100_core::event::DaemonEvent;
use x100_core::harness::ProviderFailure;
use x100_core::id::JobId;
use x100_core::job::{DaemonJob, Job, JobEvent, Schedule};
use x100_core::testing::START;
use x100_router::{cool_off, cool_off_over};

fn fold(
    events: Vec<(x100_core::party::Actor, DaemonEvent)>,
    base: &[DaemonEvent],
) -> x100_core::fold::DaemonState {
    let mut all = base.to_vec();
    all.extend(events.into_iter().map(|(_, e)| e));
    fold_daemon(all)
}

fn base() -> Vec<DaemonEvent> {
    harnesses(true)
        .into_iter()
        .map(DaemonEvent::HarnessAdded)
        .collect()
}

#[test]
fn a_cool_off_lasts_the_set_time_or_what_the_provider_asked() {
    let d = fold_daemon(base());
    let first = cool_off(
        &d,
        small(),
        ProviderFailure::KeyRefused,
        START,
        JobId::from_u128(1),
    );
    let d = fold(first.clone(), &base());
    let record = d.cooling_off(&small()).expect("cooling off");
    assert_eq!(record.cool_off.until, START + Duration::from_secs(600));
    assert_eq!(record.ends, JobId::from_u128(1));

    let asked = ProviderFailure::RateLimited {
        retry_after: Some(Duration::from_secs(3600)),
    };
    let long = cool_off(
        &fold_daemon(base()),
        mid(),
        asked,
        START,
        JobId::from_u128(2),
    );
    let d = fold(long, &base());
    assert_eq!(
        d.cooling_off(&mid()).unwrap().cool_off.until,
        START + Duration::from_secs(3600)
    );
}

#[test]
fn a_second_failure_restarts_the_cool_off_and_only_the_latest_job_ends_it() {
    let mut log = base();
    let d = fold_daemon(log.clone());
    log.extend(
        cool_off(
            &d,
            small(),
            ProviderFailure::KeyRefused,
            START,
            JobId::from_u128(1),
        )
        .into_iter()
        .map(|(_, e)| e),
    );
    let d = fold_daemon(log.clone());
    let later = START + Duration::from_secs(60);
    let again = cool_off(
        &d,
        small(),
        ProviderFailure::KeyRefused,
        later,
        JobId::from_u128(2),
    );
    assert!(
        matches!(&again[0].1, DaemonEvent::Job { job, event: JobEvent::Cancelled { .. } } if *job == JobId::from_u128(1)),
        "{again:?}"
    );
    log.extend(again.into_iter().map(|(_, e)| e));
    let d = fold_daemon(log);
    let job = |id| Job {
        id: JobId::from_u128(id),
        when: Schedule::Once { at: later },
        does: DaemonJob::EndCoolOff(small()),
    };
    assert_eq!(cool_off_over(&d, &job(1)), None);
    assert_eq!(
        cool_off_over(&d, &job(2)),
        Some(DaemonEvent::CoolOffEnded(small()))
    );
}
