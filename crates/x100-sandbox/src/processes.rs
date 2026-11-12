//! The processes a sandbox owns: started outside any driver, so a handoff leaves them running.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

use crate::{CommandLine, Error, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(pub u32);

impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "p{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub id: ProcessId,
    /// The command as it was asked for, before the sandbox wrapped it.
    pub command: CommandLine,
    /// Its standard output and error, on the host.
    pub log: PathBuf,
    /// False once it has exited by itself. A stopped process is no longer listed.
    pub running: bool,
}

struct Running {
    info: ProcessInfo,
    child: Child,
}

pub(crate) struct Processes {
    logs: PathBuf,
    table: Mutex<(u32, BTreeMap<ProcessId, Running>)>,
}

/// How long a process has to end after SIGTERM before it is killed.
const GRACE: Duration = Duration::from_secs(3);

impl Processes {
    pub(crate) fn new(logs: PathBuf) -> Self {
        Self {
            logs,
            table: Mutex::new((0, BTreeMap::new())),
        }
    }

    pub(crate) async fn start(
        &self,
        asked: CommandLine,
        wrapped: &CommandLine,
        env: &[(String, String)],
        dir: &Path,
    ) -> Result<ProcessId, Error> {
        let id = {
            let mut table = self.table.lock().expect("process table");
            table.0 += 1;
            ProcessId(table.0)
        };
        let log = self.logs.join(format!("{id}.log"));
        let out = std::fs::File::create(&log).map_err(io(format!("creating {}", log.display())))?;
        let err = out
            .try_clone()
            .map_err(io(format!("opening {}", log.display())))?;
        let child = Command::new(&wrapped.program)
            .args(&wrapped.args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(dir)
            // Its own group, so stopping it reaches the host side of the sandbox's runner.
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .kill_on_drop(true)
            .spawn()
            .map_err(io(format!("starting {}", asked.program.display())))?;
        let info = ProcessInfo {
            id,
            command: asked,
            log,
            running: true,
        };
        self.table
            .lock()
            .expect("process table")
            .1
            .insert(id, Running { info, child });
        Ok(id)
    }

    pub(crate) fn list(&self) -> Vec<ProcessInfo> {
        let mut table = self.table.lock().expect("process table");
        table
            .1
            .values_mut()
            .map(|r| {
                r.info.running = matches!(r.child.try_wait(), Ok(None));
                r.info.clone()
            })
            .collect()
    }

    pub(crate) async fn stop(&self, id: ProcessId) -> Result<(), Error> {
        let running = self
            .table
            .lock()
            .expect("process table")
            .1
            .remove(&id)
            .ok_or(Error::NoProcess(id))?;
        end(running.child).await;
        Ok(())
    }

    pub(crate) async fn stop_all(&self) {
        let all: Vec<Running> = std::mem::take(&mut self.table.lock().expect("process table").1)
            .into_values()
            .collect();
        for r in all {
            group(&r.child, Signal::SIGKILL);
            let mut child = r.child;
            let _ = child.wait().await;
        }
    }

    pub(crate) fn kill_all(&self) {
        if let Ok(table) = self.table.lock() {
            for r in table.1.values() {
                group(&r.child, Signal::SIGKILL);
            }
        }
    }
}

/// SIGTERM, a grace period, then SIGKILL, to the process's whole group on the host. Inside, the
/// runner ends everything the process started: bubblewrap's process namespace dies with it, and
/// a container's runner is told to stop its own.
async fn end(mut child: Child) {
    group(&child, Signal::SIGTERM);
    if tokio::time::timeout(GRACE, child.wait()).await.is_err() {
        group(&child, Signal::SIGKILL);
        let _ = child.wait().await;
    }
}

fn group(child: &Child, signal: Signal) {
    if let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) {
        let _ = killpg(Pid::from_raw(pid), signal);
    }
}
