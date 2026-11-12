//! Drivers are children of the daemon and must not outlive it. A daemon killed with SIGKILL cannot
//! stop them, so each driver's process is written down when it starts, and the next daemon stops
//! what is still running from that list.
//!
//! A process is identified by its id and the time it started, since ids are reused. Each process a
//! driver starts leads a process group of its own, so killing the group takes what it started with
//! it. A driver may start more than one: Codex in a terminal runs its app-server and the terminal
//! program attached to it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use x100_core::id::DriverId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    pid: u32,
    /// `ps`'s start time, to the second, which tells a reused id from ours.
    started: String,
}

/// What the daemon found left over by a daemon that died.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaped {
    /// The driver, or the sandbox, whose process it was.
    pub driver: String,
    pub pid: u32,
}

/// The driver processes the daemon has started and not yet seen end. Methods block.
pub struct ProcessRegistry {
    path: PathBuf,
    live: Mutex<BTreeMap<String, Vec<Entry>>>,
}

impl ProcessRegistry {
    /// Stops every process the file lists that is still the one that was recorded, then starts
    /// an empty list. Run before any driver is started.
    pub fn open_and_reap(path: impl Into<PathBuf>) -> std::io::Result<(Self, Vec<Reaped>)> {
        let path = path.into();
        let old: BTreeMap<String, Vec<Entry>> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let mut reaped = Vec::new();
        let old = old
            .into_iter()
            .flat_map(|(driver, entries)| entries.into_iter().map(move |e| (driver.clone(), e)));
        for (driver, entry) in old {
            // A leader that is gone is no reason to leave its group alone: while any process is in
            // a group, its id cannot be given to another process, so what is in it is ours. A
            // leader that is alive with another start time is someone else's, and is left be.
            let ours = match start_time(entry.pid) {
                None => true,
                Some(started) => started == entry.started,
            };
            if ours && kill_group(entry.pid) {
                reaped.push(Reaped {
                    driver,
                    pid: entry.pid,
                });
            }
        }
        let registry = Self {
            path,
            live: Mutex::new(BTreeMap::new()),
        };
        registry.write(&BTreeMap::new())?;
        Ok((registry, reaped))
    }

    /// Adds a process to the driver's list.
    pub fn record(&self, driver: DriverId, pid: u32) -> std::io::Result<()> {
        self.record_named(&driver.to_string(), pid)
    }

    /// Crosses off every process of the driver, once they are all gone.
    pub fn forget(&self, driver: DriverId) -> std::io::Result<()> {
        self.forget_named(&driver.to_string())
    }

    /// Adds a process to the list under `owner`, such as the process that holds a sandbox.
    pub fn record_named(&self, owner: &str, pid: u32) -> std::io::Result<()> {
        let entry = Entry {
            pid,
            started: start_time(pid).unwrap_or_default(),
        };
        let mut live = self.live.lock().expect("registry lock");
        live.entry(owner.to_owned()).or_default().push(entry);
        self.write(&live)
    }

    pub fn forget_named(&self, owner: &str) -> std::io::Result<()> {
        let mut live = self.live.lock().expect("registry lock");
        live.remove(owner);
        self.write(&live)
    }

    fn write(&self, live: &BTreeMap<String, Vec<Entry>>) -> std::io::Result<()> {
        // Renamed into place so a daemon killed mid-write leaves the last whole list.
        let part = self.path.with_extension("part");
        std::fs::write(&part, serde_json::to_vec(live).expect("a list serialises"))?;
        std::fs::rename(&part, &self.path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// When the process started, as `ps` prints it, or `None` if there is no such process. `ps` is on
/// Linux and macOS alike, which `/proc` is not.
fn start_time(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// Kills the group and waits for it to empty. True if there was anything in it.
fn kill_group(pgid: u32) -> bool {
    let Ok(id) = i32::try_from(pgid) else {
        return false;
    };
    if x100_drivers::group_is_empty(pgid) {
        return false;
    }
    let _ = killpg(Pid::from_raw(id), Signal::SIGKILL);
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until && !x100_drivers::group_is_empty(pgid) {
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}
