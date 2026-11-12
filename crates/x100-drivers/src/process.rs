//! Spawning a driver's child in a process group of its own, signalling that group, and finding
//! and stopping what the child started outside it.

use std::path::Path;
use std::process::Stdio;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

use crate::harness::CommandLine;

/// Spawns `line` with exactly `env` as its environment, in `dir`, as the leader of a new process
/// group, so that stopping it reaches everything it started that did not leave the group.
pub(crate) fn spawn(
    line: &CommandLine,
    env: &[(String, String)],
    dir: &Path,
) -> std::io::Result<Child> {
    let mut command = Command::new(&line.program);
    command
        .args(&line.args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(dir)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A last resort if the supervisor itself is dropped without stopping the driver.
        .kill_on_drop(true);
    command.spawn()
}

pub(crate) fn signal_group(pgid: u32, signal: Signal) {
    if let Ok(pgid) = i32::try_from(pgid) {
        // ESRCH means the group is already empty, which is what stopping wants.
        let _ = killpg(Pid::from_raw(pgid), signal);
    }
}

/// Whether no process is left in the process group `pgid`. A process that exited but has not been
/// reaped by its parent still counts.
pub fn group_is_empty(pgid: u32) -> bool {
    let Ok(pgid) = i32::try_from(pgid) else {
        return true;
    };
    matches!(killpg(Pid::from_raw(pgid), None), Err(Errno::ESRCH))
}

/// A process the driver started, as `ps` saw it: its id and group, and when it started, which
/// tells it from a later process given the same id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Started {
    pid: u32,
    pgid: u32,
    since: String,
}

/// Every process below `roots`, found by parent. Background commands that a harness starts in
/// sessions of their own are outside its process group but still its children.
pub(crate) async fn descendants(roots: &[u32]) -> Vec<Started> {
    let table = processes().await;
    let mut parents: Vec<u32> = roots.to_vec();
    let mut found: Vec<Started> = Vec::new();
    while let Some(parent) = parents.pop() {
        for (ppid, started) in &table {
            if *ppid == parent && !found.iter().any(|f| f.pid == started.pid) {
                parents.push(started.pid);
                found.push(started.clone());
            }
        }
    }
    found
}

/// Kills what is left of `started`, with the groups they lead, skipping any id that now belongs
/// to another process.
pub(crate) async fn kill_left(started: &[Started]) {
    if started.is_empty() {
        return;
    }
    let now = processes().await;
    for old in started {
        let still = now.iter().any(|(_, p)| p == old);
        if !still {
            continue;
        }
        if old.pgid == old.pid {
            signal_group(old.pid, Signal::SIGKILL);
        }
        if let Ok(pid) = i32::try_from(old.pid) {
            let _ = nix::sys::signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

/// `(parent, process)` for every process, from `ps`, which reads the same on Linux and macOS.
async fn processes() -> Vec<(u32, Started)> {
    let out = tokio::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid=,lstart="])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(out) = out else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            let pgid = fields.next()?.parse().ok()?;
            let since = fields.collect::<Vec<_>>().join(" ");
            Some((ppid, Started { pid, pgid, since }))
        })
        .collect()
}
