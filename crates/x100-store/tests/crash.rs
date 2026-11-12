//! Killing the writer mid-write loses no committed event.

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use x100_core::event::Seq;
use x100_core::fold::ProjectState;
use x100_store::ProjectStore;

/// Small deterministic generator so a failing run can be repeated from the printed seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn every_event_and_entry_a_killed_writer_reported_is_there_after_reopening() {
    let seed = std::env::var("X100_CRASH_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
                | 1
        });
    println!("seed {seed}");
    let mut rng = Rng(seed);
    let dir = tempfile::tempdir().unwrap();
    let ceo = x100_core::id::AgentId::from_u128(1);
    let (mut last_event, mut last_entry) = (0u64, 0u64);
    let mut kills_with_progress = 0;

    for round in 0..40 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_x100-store-appender"))
            .arg(dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        // The test reads the child's reports on a thread, so killing it is never held up.
        let (tx, rx) = mpsc::channel::<String>();
        let reader = thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });

        // The first round waits for one committed event, so there is a project to reopen however
        // slow the machine is. Later rounds may kill the child while it is still opening.
        let first = (round == 0).then(|| rx.recv().unwrap());
        thread::sleep(Duration::from_millis(40 + rng.next() % 200));
        child.kill().unwrap();
        child.wait().unwrap();
        reader.join().unwrap();

        let (mut event, mut entry) = (last_event, last_entry);
        for line in first.into_iter().chain(rx.try_iter()) {
            let (kind, n) = line.split_once(' ').unwrap();
            let n: u64 = n.parse().unwrap();
            match kind {
                "event" => {
                    assert_eq!(
                        n,
                        event + 1 + u64::from(event == 0),
                        "a gap in what it reported"
                    );
                    event = n;
                }
                "transcript" => {
                    assert_eq!(n, entry + 1);
                    entry = n;
                }
                other => panic!("unexpected report {other}"),
            }
        }
        if event > last_event {
            kills_with_progress += 1;
        }

        let (_, clock) = common::clock();
        let store = ProjectStore::open(dir.path(), clock)
            .unwrap_or_else(|e| panic!("round {round}, seed {seed}: reopen failed: {e}"));
        let stored = store.log.read_after(None, usize::MAX >> 1).unwrap();
        // Every number the child reported is stored, and the stored log folds (opening folded it).
        assert!(
            stored.len() as u64 >= event.max(1),
            "round {round}, seed {seed}: reported event {event}, stored {}",
            stored.len()
        );
        assert_eq!(store.log.last(), Some(Seq(stored.len() as u64)));
        assert_eq!(ProjectState::fold(&stored).unwrap(), store.log.state());
        let entries = store.transcripts.read(ceo, None, usize::MAX >> 1).unwrap();
        assert!(
            entries.len() as u64 >= entry,
            "round {round}, seed {seed}: reported entry {entry}, stored {}",
            entries.len()
        );
        // Positions have no holes.
        for (i, (pos, _)) in entries.iter().enumerate() {
            assert_eq!(pos.0, i as u64 + 1);
        }
        last_event = stored.len() as u64;
        last_entry = entries.len() as u64;
    }
    println!("40 kills: {last_event} events and {last_entry} transcript entries survived");
    assert!(
        kills_with_progress >= 20,
        "the child rarely got anywhere before it was killed; the test proves little"
    );
}
