//! What a decision reads: the two folded states, and what only the running daemon knows.

use std::collections::{BTreeMap, BTreeSet};

use x100_core::fold::{DaemonState, ProjectState};
use x100_core::harness::Login;
use x100_core::id::{HarnessId, MachineId};
use x100_core::time::Timestamp;

/// What this daemon can start: on which machine, and which harnesses. A harness the log remembers
/// but this run cannot start is not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub machine: MachineId,
    pub harnesses: BTreeSet<HarnessId>,
}

/// The room a driver of `harness` takes on `machine`: the machine's cap always, and the cap of the
/// login its harness runs on, if it runs on one.
pub fn slot(daemon: &DaemonState, harness: HarnessId, machine: MachineId) -> Slot {
    Slot {
        machine,
        login: daemon.harness(harness).and_then(|h| h.login.clone()),
    }
}

/// The room one live driver takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Slot {
    pub machine: MachineId,
    pub login: Option<Login>,
}

/// Live drivers across every project, by machine and by login. A machine's cap and a login's cap
/// count drivers of every project, so this is the daemon's, not one project's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Occupancy {
    machines: BTreeMap<MachineId, u32>,
    logins: BTreeMap<Login, u32>,
}

impl Occupancy {
    /// Counted from the records: every live driver of every project in `states`.
    pub fn count<'a>(
        states: impl IntoIterator<Item = &'a ProjectState>,
        daemon: &DaemonState,
    ) -> Self {
        let mut o = Occupancy::default();
        for s in states {
            for d in s.live_drivers() {
                o.take(&slot(daemon, d.driver.harness, d.driver.machine));
            }
        }
        o
    }

    pub fn take(&mut self, slot: &Slot) {
        *self.machines.entry(slot.machine).or_default() += 1;
        if let Some(login) = &slot.login {
            *self.logins.entry(login.clone()).or_default() += 1;
        }
    }

    pub fn free(&mut self, slot: &Slot) {
        if let Some(n) = self.machines.get_mut(&slot.machine) {
            *n = n.saturating_sub(1);
        }
        if let Some(n) = slot.login.as_ref().and_then(|l| self.logins.get_mut(l)) {
            *n = n.saturating_sub(1);
        }
    }

    pub fn on_machine(&self, machine: MachineId) -> u32 {
        self.machines.get(&machine).copied().unwrap_or(0)
    }

    pub fn on_login(&self, login: &Login) -> u32 {
        self.logins.get(login).copied().unwrap_or(0)
    }
}

/// Everything a decision reads.
#[derive(Clone, Copy)]
pub struct World<'a> {
    pub daemon: &'a DaemonState,
    pub project: &'a ProjectState,
    pub installed: &'a Installed,
    pub occupancy: &'a Occupancy,
    pub now: Timestamp,
}
