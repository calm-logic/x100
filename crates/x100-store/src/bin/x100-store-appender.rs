//! Test helper for the crash tests, not part of x100. Appends events and transcript entries to the
//! project store in the directory it is given, forever, and prints each committed number as it
//! learns it (every third round appends a batch, whose numbers are printed together once it commits): `event N` once the event is durable, `transcript N` likewise. The test kills it with
//! SIGKILL and checks that nothing it printed was lost.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use x100_core::action::AskMeAbout;
use x100_core::effort::EffortRange;
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId, Ulid};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::Settings;
use x100_core::time::{Clock, Timestamp};
use x100_core::transcript::{Entry, Item};
use x100_store::ProjectStore;

const BATCH: usize = 4;

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after 1970")
            .as_millis();
        Timestamp::from_millis(ms as u64)
    }
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("a project directory"));
    let store = ProjectStore::open(&dir, Arc::new(SystemClock)).expect("opens");
    let ceo = AgentId::from_u128(1);
    if store.log.last().is_none() {
        let created = ProjectEvent::Created {
            ceo,
            charter: "survive".into(),
            effort: EffortRange::FULL,
            settings: Settings {
                ask_me_about: AskMeAbout::Nothing,
                budget: Money::from_dollars(10),
                lab_share: Percent::new(10).expect("a percentage"),
                allowed: Default::default(),
            },
        };
        store.log.append(Actor::Builder, created).expect("created");
    }
    let driver = DriverId::new(Ulid::from_parts(1, 1));
    let mut out = std::io::stdout().lock();
    let mut i = 0usize;
    loop {
        i += 1;
        // Rows of very different sizes, so a kill lands in the middle of writes of every shape.
        let url = format!("http://preview/{i}/{}", "x".repeat(i * 37 % 4000));
        // Every third round is a batch, tagged so the test can tell whether all of it is there.
        let events = if i.is_multiple_of(3) {
            (0..BATCH)
                .map(|k| {
                    let url = format!("batch/{i}/{k}/{BATCH}/{}", "y".repeat(i * 53 % 3000));
                    (Actor::Daemon, ProjectEvent::PreviewSet { url })
                })
                .collect()
        } else {
            vec![(Actor::Daemon, ProjectEvent::PreviewSet { url })]
        };
        for e in store.log.append_all(events).expect("appends") {
            writeln!(out, "event {}", e.seq.0).expect("stdout");
        }
        out.flush().expect("flush");
        let entry = Entry {
            at: SystemClock.now(),
            driver,
            item: Item::Text {
                text: format!("line {i}"),
            },
        };
        let pos = store.transcripts.append(ceo, entry).expect("appends");
        writeln!(out, "transcript {}", pos.0).expect("stdout");
        out.flush().expect("flush");
    }
}
