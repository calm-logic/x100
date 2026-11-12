//! The event log: replay, late subscribers, concurrent appends, corruption.

mod common;

use std::sync::Arc;
use std::thread;

use rusqlite::Connection;
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent, Seq};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::party::Actor;
use x100_core::settings::DaemonSetting;
use x100_core::testing::{daemon_log, fernleaf};
use x100_core::time::Timestamp;
use x100_store::{DaemonLog, Error, ProjectStore, database_path, open_daemon_log};

/// Appends a recorded log, setting the clock to each event's own time first.
fn replay<E: Clone + std::fmt::Debug + PartialEq, S>(
    log: &x100_store::EventLog<S>,
    clock: &x100_core::time::FakeClock,
    recorded: &[Envelope<E>],
) where
    S: x100_store::Folded<Event = E>,
{
    for e in recorded {
        clock.set(e.at).expect("the log only moves forward");
        let got = log.append(e.by, e.event.clone()).expect("a valid event");
        assert_eq!(&got, e);
    }
}

#[test]
fn a_recorded_project_log_replays_to_identical_tab_state_after_reopening() {
    let recorded = fernleaf().log;
    assert!(recorded.len() > 600);
    let dir = tempfile::tempdir().unwrap();
    let (fake, clock) = common::clock();
    {
        let store = ProjectStore::open(dir.path(), clock.clone()).unwrap();
        replay(&store.log, &fake, &recorded);
    }
    let reopened = ProjectStore::open(dir.path(), clock).unwrap();
    let expected = ProjectState::fold(&recorded).unwrap();
    assert_eq!(reopened.log.state(), expected);
    // Byte for byte, as a client would receive it.
    assert_eq!(
        serde_json::to_string(&reopened.log.state()).unwrap(),
        serde_json::to_string(&expected).unwrap()
    );
    let all = reopened.log.read_after(None, usize::MAX >> 1).unwrap();
    assert_eq!(all, recorded);
}

#[test]
fn a_recorded_daemon_log_replays_to_identical_state() {
    let recorded = daemon_log(Timestamp::from_millis(1_000));
    let dir = tempfile::tempdir().unwrap();
    let (fake, clock) = common::clock();
    {
        let log = open_daemon_log(dir.path(), clock.clone()).unwrap();
        replay(&log, &fake, &recorded);
    }
    let reopened = open_daemon_log(dir.path(), clock).unwrap();
    assert_eq!(reopened.state(), DaemonState::fold(&recorded).unwrap());
}

#[test]
fn reading_pages_forward_and_newest_first() {
    let recorded = fernleaf().log;
    let dir = tempfile::tempdir().unwrap();
    let (fake, clock) = common::clock();
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    replay(&store.log, &fake, &recorded[..50]);

    let first = store.log.read_after(None, 10).unwrap();
    assert_eq!(first, recorded[..10]);
    let next = store.log.read_after(Some(first[9].seq), 5).unwrap();
    assert_eq!(next, recorded[10..15]);

    let newest = store.log.read_before(None, 4).unwrap();
    let seqs: Vec<u64> = newest.iter().map(|e| e.seq.0).collect();
    assert_eq!(seqs, [50, 49, 48, 47]);
    let older = store.log.read_before(Some(Seq(47)), 3).unwrap();
    let seqs: Vec<u64> = older.iter().map(|e| e.seq.0).collect();
    assert_eq!(seqs, [46, 45, 44]);
    assert!(store.log.read_before(Some(Seq(1)), 10).unwrap().is_empty());
}

#[test]
fn an_event_the_fold_refuses_is_not_stored_and_the_log_carries_on() {
    let recorded = fernleaf().log;
    let dir = tempfile::tempdir().unwrap();
    let (fake, clock) = common::clock();
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    replay(&store.log, &fake, &recorded[..20]);

    // A second Created is never valid.
    let again = recorded[0].event.clone();
    assert!(matches!(
        store.log.append(Actor::Builder, again),
        Err(Error::Rejected(_))
    ));
    assert_eq!(store.log.last(), Some(Seq(20)));
    assert_eq!(store.log.read_after(None, 100).unwrap().len(), 20);

    fake.set(recorded[20].at).unwrap();
    let e = store
        .log
        .append(recorded[20].by, recorded[20].event.clone())
        .unwrap();
    assert_eq!(e.seq, Seq(21));
    assert_eq!(
        store.log.state(),
        ProjectState::fold(&recorded[..21]).unwrap()
    );
}

#[test]
fn a_stored_event_the_fold_rejects_is_reported_as_corruption_with_its_number() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, clock) = common::clock();
    let recorded = fernleaf().log;
    {
        let store = ProjectStore::open(dir.path(), clock.clone()).unwrap();
        replay(&store.log, &fake, &recorded[..5]);
    }
    // Written behind the store's back: a second creation of the project.
    let conn = Connection::open(database_path(dir.path())).unwrap();
    conn.execute(
        "INSERT INTO events (seq, at, by, event) VALUES (6, 1, ?1, ?2)",
        (
            serde_json::to_string(&Actor::Builder).unwrap(),
            serde_json::to_string(&recorded[0].event).unwrap(),
        ),
    )
    .unwrap();
    drop(conn);

    match ProjectStore::open(dir.path(), clock) {
        Err(Error::Corrupt(e)) => assert_eq!(e.seq, Seq(6)),
        other => panic!("expected corruption at 6, got {:?}", other.err()),
    }
}

#[test]
fn a_second_open_of_the_same_store_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let _first = ProjectStore::open(dir.path(), clock.clone()).unwrap();
    assert!(matches!(
        ProjectStore::open(dir.path(), clock),
        Err(Error::Locked(_))
    ));
}

fn setting(n: u32) -> DaemonEvent {
    DaemonEvent::SettingChanged(DaemonSetting::RetireAfterTasks(n))
}

#[test]
fn a_subscriber_that_connects_while_others_append_gets_a_snapshot_then_every_later_event_once() {
    const WRITERS: u32 = 4;
    const EACH: u32 = 400;
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let log: Arc<DaemonLog> = Arc::new(open_daemon_log(dir.path(), clock).unwrap());

    let writers: Vec<_> = (0..WRITERS)
        .map(|w| {
            let log = log.clone();
            thread::spawn(move || {
                for i in 0..EACH {
                    log.append(Actor::Daemon, setting(w * 1000 + i)).unwrap();
                }
            })
        })
        .collect();

    // Several subscribers arrive at different moments of the run.
    let mut subscribers = Vec::new();
    for _ in 0..6 {
        thread::yield_now();
        while log.last().map_or(0, |s| s.0) < subscribers.len() as u64 * 200 {
            thread::yield_now();
        }
        subscribers.push(log.snapshot_and_follow());
    }
    for w in writers {
        w.join().unwrap();
    }
    let total = u64::from(WRITERS * EACH);
    let everything = log.read_after(None, usize::MAX >> 1).unwrap();
    assert_eq!(everything.len() as u64, total);

    for (snapshot, mut rx) in subscribers {
        let at = snapshot.seq().map_or(0, |s| s.0);
        // The snapshot is exactly the fold of the log up to its sequence number.
        assert_eq!(
            snapshot,
            DaemonState::fold(&everything[..at as usize]).unwrap()
        );
        let mut got = Vec::new();
        while let Ok(e) = rx.try_recv() {
            got.push(e);
        }
        let seqs: Vec<u64> = got.iter().map(|e| e.seq.0).collect();
        let expected: Vec<u64> = (at + 1..=total).collect();
        assert_eq!(seqs, expected, "snapshot at {at}: a gap or a repeat");
        assert_eq!(got, everything[at as usize..]);
    }
}

#[test]
fn following_from_a_remembered_number_replays_what_was_missed_then_goes_live() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let log = open_daemon_log(dir.path(), clock).unwrap();
    for i in 0..10 {
        log.append(Actor::Daemon, setting(i)).unwrap();
    }
    let mut rx = log.follow_after(Some(Seq(7))).unwrap();
    log.append(Actor::Daemon, setting(10)).unwrap();
    let seqs: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|e| e.seq.0)
        .collect();
    assert_eq!(seqs, [8, 9, 10, 11]);
}

#[test]
fn a_dropped_subscriber_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let log = open_daemon_log(dir.path(), clock).unwrap();
    let (_, rx) = log.snapshot_and_follow();
    drop(rx);
    log.append(Actor::Daemon, setting(1)).unwrap();
    log.append(Actor::Daemon, setting(2)).unwrap();
    assert_eq!(log.last(), Some(Seq(2)));
}

#[test]
fn project_events_carry_their_envelope_through_the_store_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, clock) = common::clock();
    let recorded = fernleaf().log;
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    replay(&store.log, &fake, &recorded[..3]);
    let e: Envelope<ProjectEvent> = store.log.read_after(Some(Seq(2)), 1).unwrap().remove(0);
    assert_eq!(e, recorded[2]);
}
