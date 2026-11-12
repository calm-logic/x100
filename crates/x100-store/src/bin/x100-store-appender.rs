//! Test helper for the crash tests, not part of x100. Appends events and transcript entries to the
//! project store in the directory it is given, forever, and prints each committed number as it
//! learns it: `event N` once the event is durable, `transcript N` likewise. The test kills it with
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
use x100_store::ProjectStore;
use x100_store::transcript::{Entry, Item};

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
            name: "crash test".into(),
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
        let e = store
            .log
            .append(Actor::Daemon, ProjectEvent::PreviewSet { url })
            .expect("appends");
        writeln!(out, "event {}", e.seq.0).expect("stdout");
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
