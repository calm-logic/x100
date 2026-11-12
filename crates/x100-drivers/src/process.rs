//! Spawning a driver's child in a process group of its own, and signalling that group.

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
