//! The runner on the system clock. The happy path waits about 0.2 s; the timeouts only bound a hang.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use tokio::sync::mpsc::unbounded_channel;
use tokio::time::timeout;
use x100_core::party::Actor;
use x100_core::time::{Clock, Timestamp};
use x100_jobs::SystemClock;

/// Real time that cannot step. This machine's wall clock (WSL2) was measured stepping by 16 s
/// within half a minute, which would make a test of 100 ms waits fail at random. `SystemClock`
/// is checked against the wall clock in its own unit test.
struct Steady {
    wall: Timestamp,
    since: Instant,
}

impl Clock for Steady {
    fn now(&self) -> Timestamp {
        self.wall + self.since.elapsed()
    }
}

#[tokio::test]
async fn a_job_a_short_time_ahead_fires_and_an_earlier_one_wakes_the_runner() {
    let started = Instant::now();
    let dir = tempfile::tempdir().unwrap();
    let (_log, sched) = open_project_on(
        dir.path(),
        Arc::new(Steady {
            wall: SystemClock.now(),
            since: Instant::now(),
        }),
    );
    let (tx, mut fired) = unbounded_channel();
    let runner = tokio::spawn(sched.clone().run(tx));

    // A job 100 ms ahead fires, and not much before then.
    sched
        .schedule_after(
            Actor::Builder,
            Duration::from_millis(100),
            wake("soon"),
            no_minimum(),
        )
        .unwrap();
    let first = timeout(Duration::from_secs(2), fired.recv())
        .await
        .expect("the job fires")
        .unwrap();
    assert_eq!(note(&first.job.does), "soon");
    assert!(started.elapsed() >= Duration::from_millis(100));

    // The runner is now asleep until a job a minute away. One scheduled sooner wakes it.
    sched
        .schedule_after(
            Actor::Builder,
            Duration::from_secs(60),
            wake("far"),
            no_minimum(),
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    sched
        .schedule_after(
            Actor::Builder,
            Duration::from_millis(60),
            wake("earlier"),
            no_minimum(),
        )
        .unwrap();
    let second = timeout(Duration::from_secs(2), fired.recv())
        .await
        .expect("the earlier job wakes the runner")
        .unwrap();
    assert_eq!(note(&second.job.does), "earlier");

    // Closing the channel ends the runner at its next firing.
    drop(fired);
    sched
        .schedule_after(Actor::Builder, Duration::ZERO, wake("nudge"), no_minimum())
        .unwrap();
    timeout(Duration::from_secs(2), runner)
        .await
        .expect("the runner stops")
        .unwrap()
        .unwrap();
}
