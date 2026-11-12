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

#[tokio::test]
async fn a_job_appended_in_a_batch_fires_promptly_once_the_runner_is_woken() {
    use x100_core::event::ProjectEvent;
    use x100_core::id::JobId;
    use x100_core::job::Job;

    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Steady {
        wall: SystemClock.now(),
        since: Instant::now(),
    });
    let (log, sched) = open_project_on(dir.path(), clock.clone());
    let (tx, mut fired) = unbounded_channel();
    let runner = tokio::spawn(sched.clone().run(tx));

    // Once a job has fired and nothing else is scheduled, the runner sleeps for its longest sleep.
    // The pause lets it reach that sleep; a job appended before then it would see by itself.
    sched
        .schedule_after(Actor::Builder, Duration::ZERO, wake("now"), no_minimum())
        .unwrap();
    let now = timeout(Duration::from_secs(2), fired.recv()).await;
    assert_eq!(note(&now.expect("the job fires").unwrap().job.does), "now");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Two jobs go into the log in one append, as the orchestrator writes them, so `schedule`
    // never sees them.
    let job = |n: u128, delay: Duration, what: &str| Job {
        id: JobId::from_u128(n),
        when: x100_core::job::Schedule::Once {
            at: clock.now() + delay,
        },
        does: wake(what),
    };
    let started = Instant::now();
    log.append_all(vec![
        (
            Actor::Builder,
            ProjectEvent::JobScheduled(job(1, Duration::from_millis(150), "soon")),
        ),
        (
            Actor::Builder,
            ProjectEvent::JobScheduled(job(2, Duration::from_secs(3600), "later")),
        ),
    ])
    .unwrap();
    sched.wake();

    let first = timeout(Duration::from_secs(5), fired.recv())
        .await
        .expect("the job fires well before the runner's next look at the clock")
        .unwrap();
    assert_eq!(note(&first.job.does), "soon");
    assert!(started.elapsed() >= Duration::from_millis(150));

    drop(fired);
    sched.wake();
    timeout(Duration::from_secs(2), runner)
        .await
        .expect("the runner stops")
        .unwrap()
        .unwrap();
}
