//! The scheduler on the fake clock: nothing fires until the test moves the clock.

mod common;

use common::*;
use x100_core::fold::ProjectState;
use x100_core::job::JobState;
use x100_core::party::Actor;
use x100_core::time::FakeClock;
use x100_jobs::Error;

fn setup() -> (tempfile::TempDir, FakeClock) {
    (tempfile::tempdir().unwrap(), FakeClock::new(at(START)))
}

#[test]
fn a_one_off_job_fires_once_at_its_time_and_not_before() {
    let (dir, clock) = setup();
    let (log, sched) = open_project(dir.path(), &clock);
    let id = sched
        .schedule(Actor::Builder, once(START + 1_000), wake("a"), no_minimum())
        .unwrap();

    assert!(
        fired(&sched).is_empty(),
        "nothing fires while the clock stands still"
    );
    clock.advance(ms(999));
    assert!(fired(&sched).is_empty());

    clock.advance(ms(1));
    let got = fired(&sched);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].job.id, id);
    assert_eq!(note(&got[0].job.does), "a");
    assert_eq!(
        (got[0].due, got[0].at),
        (at(START + 1_000), at(START + 1_000))
    );

    clock.advance(ms(60_000));
    assert!(fired(&sched).is_empty(), "a one-off job never fires again");
    log.with_state(|s: &ProjectState| {
        assert_eq!(s.job(id).unwrap().state, JobState::Done);
        assert_eq!(s.job(id).unwrap().fired, 1);
    });
}

#[test]
fn a_job_scheduled_after_a_delay_fires_that_long_from_now() {
    let (dir, clock) = setup();
    let (_log, sched) = open_project(dir.path(), &clock);
    sched
        .schedule_after(Actor::Builder, ms(250), wake("later"), no_minimum())
        .unwrap();
    clock.advance(ms(249));
    assert!(fired(&sched).is_empty());
    clock.advance(ms(1));
    assert_eq!(fired(&sched).len(), 1);
}

#[test]
fn a_repeating_job_fires_at_each_interval() {
    let (dir, clock) = setup();
    let (_log, sched) = open_project(dir.path(), &clock);
    let id = sched
        .schedule(
            Actor::Builder,
            repeat(START + 100, 100),
            wake("r"),
            no_minimum(),
        )
        .unwrap();

    clock.advance(ms(99));
    assert!(fired(&sched).is_empty());
    for round in 1..=5 {
        clock.advance(ms(if round == 1 { 1 } else { 100 }));
        let got = fired(&sched);
        assert_eq!(got.len(), 1, "round {round}");
        assert_eq!(got[0].job.id, id);
        assert_eq!(got[0].at, at(START + 100 * round));
        // Not twice at the same instant.
        assert!(fired(&sched).is_empty());
    }
    assert_eq!(sched.next_due(), Some(at(START + 600)));
}

#[test]
fn a_cancelled_job_never_fires() {
    let (dir, clock) = setup();
    let (_log, sched) = open_project(dir.path(), &clock);
    let one_off = sched
        .schedule(Actor::Builder, once(START + 100), wake("a"), no_minimum())
        .unwrap();
    let repeating = sched
        .schedule(
            Actor::Builder,
            repeat(START + 100, 100),
            wake("b"),
            no_minimum(),
        )
        .unwrap();
    let kept = sched
        .schedule(Actor::Builder, once(START + 100), wake("c"), no_minimum())
        .unwrap();
    sched.cancel(Actor::Builder, one_off).unwrap();
    sched.cancel(Actor::Builder, repeating).unwrap();

    clock.advance(ms(10_000));
    assert_eq!(fired_ids(&sched), [kept]);
    clock.advance(ms(10_000));
    assert!(fired(&sched).is_empty());
    assert_eq!(sched.next_due(), None);
}

#[test]
fn a_job_that_has_ended_cannot_be_cancelled() {
    let (dir, clock) = setup();
    let (_log, sched) = open_project(dir.path(), &clock);
    let id = sched
        .schedule(Actor::Builder, once(START), wake("a"), no_minimum())
        .unwrap();
    assert_eq!(fired(&sched).len(), 1);
    let err = sched.cancel(Actor::Builder, id).unwrap_err();
    assert!(
        matches!(&err, Error::Store(e) if matches!(**e, x100_store::Error::Rejected(_))),
        "{err}"
    );
}

#[test]
fn jobs_due_together_fire_in_due_order() {
    let (dir, clock) = setup();
    let (_log, sched) = open_project(dir.path(), &clock);
    let late = sched
        .schedule(
            Actor::Builder,
            once(START + 300),
            wake("late"),
            no_minimum(),
        )
        .unwrap();
    let early = sched
        .schedule(
            Actor::Builder,
            once(START + 100),
            wake("early"),
            no_minimum(),
        )
        .unwrap();
    let middle = sched
        .schedule(
            Actor::Builder,
            repeat(START + 200, 1_000),
            wake("mid"),
            no_minimum(),
        )
        .unwrap();
    clock.advance(ms(1_000));
    assert_eq!(fired_ids(&sched), [early, middle, late]);
}

#[test]
fn a_job_due_across_a_restart_fires_once_not_once_per_missed_interval() {
    let (dir, clock) = setup();
    let (log, sched) = open_project(dir.path(), &clock);
    let repeating = sched
        .schedule(
            Actor::Builder,
            repeat(START + 1_000, 1_000),
            wake("r"),
            no_minimum(),
        )
        .unwrap();
    let one_off = sched
        .schedule(Actor::Builder, once(START + 2_500), wake("o"), no_minimum())
        .unwrap();
    clock.advance(ms(1_000));
    assert_eq!(fired_ids(&sched), [repeating]);

    // The daemon stops, and 50 intervals pass.
    drop((log, sched));
    clock.advance(ms(50_400));
    let (log, sched) = open_project(dir.path(), &clock);

    let got = fired(&sched);
    assert_eq!(
        got.iter().map(|f| f.job.id).collect::<Vec<_>>(),
        [repeating, one_off]
    );
    assert_eq!(
        got[0].due,
        at(START + 2_000),
        "it was due since the next interval"
    );
    assert_eq!(got[0].at, at(START + 51_400));
    assert!(
        fired(&sched).is_empty(),
        "nothing fires again for the missed intervals"
    );
    log.with_state(|s: &ProjectState| assert_eq!(s.job(repeating).unwrap().fired, 2));

    // It carries on from its series, not from the time it was late.
    assert_eq!(sched.next_due(), Some(at(START + 52_000)));
    clock.advance(ms(600));
    assert_eq!(fired_ids(&sched), [repeating]);
}

#[test]
fn a_job_cancelled_before_a_restart_does_not_fire_after_it() {
    let (dir, clock) = setup();
    let (log, sched) = open_project(dir.path(), &clock);
    let id = sched
        .schedule(Actor::Builder, once(START + 100), wake("a"), no_minimum())
        .unwrap();
    sched.cancel(Actor::Builder, id).unwrap();
    drop((log, sched));
    clock.advance(ms(1_000));
    let (_log, sched) = open_project(dir.path(), &clock);
    assert!(fired(&sched).is_empty());
}

#[test]
fn a_repeat_shorter_than_the_minimum_is_refused_and_writes_nothing() {
    let (dir, clock) = setup();
    let (log, sched) = open_project(dir.path(), &clock);
    let before = log.last();
    let err = sched
        .schedule(
            Actor::Builder,
            repeat(START, 299_999),
            wake("a"),
            every(300_000),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        Error::RepeatTooShort { asked, minimum }
            if asked == every(299_999) && minimum == every(300_000)
    ));
    assert_eq!(log.last(), before);

    // Exactly the minimum is allowed, and a one-off is not a repeat.
    sched
        .schedule(
            Actor::Builder,
            repeat(START, 300_000),
            wake("b"),
            every(300_000),
        )
        .unwrap();
    sched
        .schedule(Actor::Builder, once(START), wake("c"), every(300_000))
        .unwrap();
}

#[test]
fn the_daemons_own_jobs_are_not_exempt_from_the_minimum() {
    let (dir, clock) = setup();
    let (log, sched) = open_daemon(dir.path(), &clock);
    let before = log.last();
    let err = sched
        .schedule(
            Actor::Daemon,
            repeat(START, 1_000),
            cool_off_end(),
            every(300_000),
        )
        .unwrap_err();
    assert!(matches!(err, Error::RepeatTooShort { .. }));
    assert_eq!(log.last(), before);
}

#[test]
fn the_same_scheduler_serves_the_daemons_log() {
    let (dir, clock) = setup();
    let (log, sched) = open_daemon(dir.path(), &clock);
    let id = sched
        .schedule_after(Actor::Daemon, ms(600_000), cool_off_end(), every(300_000))
        .unwrap();
    clock.advance(ms(599_999));
    assert!(fired(&sched).is_empty());
    clock.advance(ms(1));
    let got = fired(&sched);
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].job.id, &got[0].job.does), (id, &cool_off_end()));

    drop((log, sched));
    let (_log, sched) = open_daemon(dir.path(), &clock);
    clock.advance(ms(10_000_000));
    assert!(fired(&sched).is_empty());
}

#[test]
fn a_job_is_handed_over_only_after_it_is_in_the_log() {
    let (dir, clock) = setup();
    let (log, sched) = open_project(dir.path(), &clock);
    sched
        .schedule(Actor::Builder, once(START), wake("a"), no_minimum())
        .unwrap();
    let mut seen = 0;
    sched
        .fire_due(|f| {
            seen += 1;
            log.with_state(|s: &ProjectState| {
                assert_eq!(s.job(f.job.id).unwrap().state, JobState::Done);
            });
        })
        .unwrap();
    assert_eq!(seen, 1);
}

#[test]
fn two_schedulers_over_one_log_fire_a_job_once_between_them() {
    let (dir, clock) = setup();
    let (_log, first) = open_project(dir.path(), &clock);
    let second = first.clone();
    first
        .schedule(Actor::Builder, once(START), wake("a"), no_minimum())
        .unwrap();
    let total = fired(&first).len() + fired(&second).len();
    assert_eq!(total, 1);
}
