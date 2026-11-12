//! Agents at work. This milestone is the turn loop: when an agent has inbox items and no running
//! driver, start one in the agent's worktree, give it a turn, record what it does, and mark the
//! items handled only when the turn completes. A warm driver takes the next items as a further
//! turn.
//!
//! The seams the rest of orchestration builds on:
//! - [`DriverChooser`] says which harness, model and effort an agent's next driver gets.
//! - [`tools::NoTools`] is what the fake harness's tool calls reach until the daemon serves tools.
//! - [`pack`] builds a driver's first instructions and the text of a turn from the log.
//! - `driver::Actor::finish_turn` is where a completed turn is recorded, and where reports, waits
//!   and the nudge for a turn that neither reported nor waited will go.

// x100_store::Error is over 128 bytes and every store call returns it; boxing it is the store's change.
#![allow(clippy::result_large_err)]

mod builder;
mod choose;
mod driver;
mod ids;
pub mod pack;
mod processes;
mod project;
mod recover;
mod store;
pub mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};
use x100_core::id::{DriverId, HarnessId, ProjectId};
use x100_core::settings::DaemonSettings;
use x100_core::time::Clock;
use x100_drivers::Harness;
use x100_store::ProjectStore;

pub use builder::{Refused, deliver_job, raise_task, send_brief, send_chat};
pub use choose::{Choice, DriverChooser};
pub use ids::new_id;
pub use processes::{ProcessRegistry, Reaped};
pub use recover::recover;

/// Everything the orchestrator is built from.
pub struct Config {
    pub clock: Arc<dyn Clock>,
    pub harnesses: HashMap<HarnessId, Arc<dyn Harness>>,
    pub chooser: Arc<dyn DriverChooser>,
    /// Read when a driver starts, so a changed setting reaches the next driver.
    pub settings: Arc<dyn Fn() -> DaemonSettings + Send + Sync>,
    /// Where each driver's process is written down, so a killed daemon's drivers can be found.
    pub processes: Arc<ProcessRegistry>,
}

pub(crate) struct Shared {
    pub clock: Arc<dyn Clock>,
    pub harnesses: HashMap<HarnessId, Arc<dyn Harness>>,
    pub chooser: Arc<dyn DriverChooser>,
    pub settings: Arc<dyn Fn() -> DaemonSettings + Send + Sync>,
    pub processes: Arc<ProcessRegistry>,
}

/// Why a driver could not be stopped by [`Orchestrator::stop_driver`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StopError {
    #[error("no such driver")]
    NotFound,
    #[error("the driver has already ended")]
    NotLive,
}

struct ProjectHandle {
    ctl: mpsc::UnboundedSender<project::Ctl>,
    task: tokio::task::JoinHandle<()>,
}

/// Runs the turn loop of every attached project.
pub struct Orchestrator {
    shared: Arc<Shared>,
    projects: Mutex<HashMap<ProjectId, ProjectHandle>>,
}

impl Orchestrator {
    pub fn new(config: Config) -> Self {
        Self {
            shared: Arc::new(Shared {
                clock: config.clock,
                harnesses: config.harnesses,
                chooser: config.chooser,
                settings: config.settings,
                processes: config.processes,
            }),
            projects: Mutex::new(HashMap::new()),
        }
    }

    /// Starts the turn loop of a project whose store is open. Any driver the log shows as live
    /// must have been dealt with by [`recover`] first. An agent that already has inbox items gets
    /// a driver at once.
    pub fn attach(&self, project: ProjectId, store: ProjectStore) {
        let (ctl, task) = project::spawn(Arc::clone(&self.shared), store);
        self.projects
            .lock()
            .expect("projects lock")
            .insert(project, ProjectHandle { ctl, task });
    }

    /// Stops a driver the builder chose, and records it. Whatever turn it was in is lost and its
    /// inbox items go to the next driver. Returns once the driver's process is gone.
    pub async fn stop_driver(&self, project: ProjectId, driver: DriverId) -> Result<(), StopError> {
        let ctl = self
            .projects
            .lock()
            .expect("projects lock")
            .get(&project)
            .map(|p| p.ctl.clone())
            .ok_or(StopError::NotFound)?;
        let (ack, done) = oneshot::channel();
        ctl.send(project::Ctl::Stop { driver, ack })
            .map_err(|_| StopError::NotLive)?;
        match done.await {
            Ok(true) => Ok(()),
            _ => Err(StopError::NotLive),
        }
    }

    /// Stops every driver, recording a warm one as ended and a running one as failed, both
    /// because the daemon stopped, and returns when no driver process of ours is left.
    pub async fn shutdown(&self) {
        let handles: Vec<_> = self
            .projects
            .lock()
            .expect("projects lock")
            .drain()
            .map(|(_, p)| p)
            .collect();
        for p in handles {
            let (ack, done) = oneshot::channel();
            if p.ctl.send(project::Ctl::Shutdown(ack)).is_ok() {
                let _ = done.await;
            }
            p.task.abort();
        }
    }
}
