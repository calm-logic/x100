//! The room live drivers take on the machine and on provider logins, for every project at once.
//!
//! A machine's cap and a login's cap count the drivers of every project, so each project's turn
//! loop allocates under one lock here: two projects can never both take the last room. A driver's
//! room is taken when the router picks it and given back when its task ends, however it ends. The
//! log shows the same drivers as live, since drivers do not outlive the daemon.

use std::collections::BTreeMap;
use std::sync::Mutex;

use tokio::sync::watch;
use x100_core::id::DriverId;
use x100_router::{Occupancy, Slot};

pub(crate) struct Fleet {
    slots: Mutex<BTreeMap<DriverId, Slot>>,
    /// Bumped whenever room frees or the daemon changes something that could let a waiting agent
    /// find a driver, so every project's loop tries its waiting agents again.
    changed: watch::Sender<u64>,
}

impl Fleet {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(BTreeMap::new()),
            changed: watch::channel(0).0,
        }
    }

    /// Runs `decide` over the room in use, and takes the room it returns, under one lock. Blocks
    /// while another project allocates.
    pub fn allocate<R>(
        &self,
        decide: impl FnOnce(&Occupancy) -> (R, Option<(DriverId, Slot)>),
    ) -> R {
        let mut slots = self.slots.lock().expect("fleet lock");
        let mut occupancy = Occupancy::default();
        for slot in slots.values() {
            occupancy.take(slot);
        }
        let (r, taken) = decide(&occupancy);
        if let Some((driver, slot)) = taken {
            slots.insert(driver, slot);
        }
        r
    }

    pub fn release(&self, driver: DriverId) {
        let freed = self
            .slots
            .lock()
            .expect("fleet lock")
            .remove(&driver)
            .is_some();
        if freed {
            self.changed();
        }
    }

    pub fn changed(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }
}
