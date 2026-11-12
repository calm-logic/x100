//! Agents at work as an organisation: the CEO, leads and workers, delegation down the tree and
//! reports back up, the inbox, waits, jobs, and the start pack every driver starts from.
//!
//! - Each project has a turn loop ([`project`]) that gives an agent's waiting inbox items to its
//!   warm driver, or starts one, and a driver task per driver ([`driver`]) that records its turns.
//! - Agents act through the daemon's tools, which arrive at the [`Desk`] whatever carries them.
//!   [`tools`] decides each call from the folded state alone; the desk records it as an action,
//!   does the git work it needs, and appends what happened.
//! - Every append goes through one writer per project, which closes the waits its events end
//!   ([`settle`]), so the rule in `Wait::ended_by` has one home.
//!
//! The seams later milestones attach to, each with today's trivial behaviour:
//! - [`DriverChooser`] says which harness, model and effort an agent's next driver gets (M8).
//! - [`Sandbox`] wraps a driver's command line to run it in the agent's sandbox (M10).
//! - [`Gate`] classifies each action and allows or holds it (M12).

mod branches;
mod builder;
mod choose;
mod ctx;
mod desk;
mod driver;
mod gate;
mod ids;
mod jobs;
pub mod pack;
mod processes;
mod project;
mod recover;
mod sandbox;
mod settle;
mod store;
mod tools;
mod turns;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId, FileId, HarnessId, JobId, MessageId, ProjectId, TaskId};
use x100_core::job::{Job, ProjectJob};
use x100_core::settings::DaemonSettings;
use x100_core::time::{Clock, Timestamp};
use x100_core::wait::Wait;
use x100_drivers::Harness;
use x100_jobs::Scheduler;
use x100_store::ProjectStore;

pub use builder::Refused;
pub use choose::{Choice, DriverChooser};
pub use desk::Desk;
pub use gate::{AllowAll, Gate, Ruling};
pub use ids::new_id;
pub use processes::{ProcessRegistry, Reaped};
pub use recover::recover;
pub use sandbox::{NoSandbox, Sandbox};

use ctx::ProjectCtx;
use store::blocking;

/// The daemon's settings as they are now, read whenever one is needed so a change reaches the
/// next decision.
pub type Settings = Arc<dyn Fn() -> DaemonSettings + Send + Sync>;

/// Everything the orchestrator is built from.
pub struct Config {
    pub clock: Arc<dyn Clock>,
    pub harnesses: HashMap<HarnessId, Arc<dyn Harness>>,
    pub chooser: Arc<dyn DriverChooser>,
    pub settings: Settings,
    /// Where each driver's process is written down, so a killed daemon's drivers can be found.
    pub processes: Arc<ProcessRegistry>,
    /// Where drivers' tool calls arrive. The harnesses are given it too, so it is made first.
    pub desk: Arc<Desk>,
    pub sandbox: Arc<dyn Sandbox>,
    /// One turn at a time in each project, the agent whose oldest waiting item came first going
    /// next. Agents' processes run concurrently, so only this makes a log made on the fake
    /// harness the same on every run. The daemon runs turns side by side.
    pub serial: bool,
}

pub(crate) struct Shared {
    pub clock: Arc<dyn Clock>,
    pub harnesses: HashMap<HarnessId, Arc<dyn Harness>>,
    pub chooser: Arc<dyn DriverChooser>,
    pub settings: Settings,
    pub processes: Arc<ProcessRegistry>,
    pub desk: Arc<Desk>,
    pub sandbox: Arc<dyn Sandbox>,
    pub serial: bool,
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
    ctx: Arc<ProjectCtx>,
    ctl: mpsc::UnboundedSender<project::Ctl>,
    task: tokio::task::JoinHandle<()>,
}

/// Runs the turn loop of every attached project. Clones share it.
#[derive(Clone)]
pub struct Orchestrator {
    shared: Arc<Shared>,
    projects: Arc<Mutex<HashMap<ProjectId, ProjectHandle>>>,
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
                desk: config.desk,
                sandbox: config.sandbox,
                serial: config.serial,
            }),
            projects: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Starts the turn loop of a project whose store is open. Any driver the log shows as live
    /// must have been dealt with by [`recover`] first. An agent that already has inbox items gets
    /// a driver at once. `jobs` is the scheduler that fires the project's jobs, told of each job
    /// the orchestrator appends.
    pub fn attach(&self, project: ProjectId, store: ProjectStore, jobs: Scheduler<ProjectState>) {
        let ctx = ProjectCtx::new(project, store, Arc::clone(&self.shared.clock), jobs);
        let (ctl, task) = project::spawn(Arc::clone(&self.shared), Arc::clone(&ctx));
        self.projects
            .lock()
            .expect("projects lock")
            .insert(project, ProjectHandle { ctx, ctl, task });
    }

    fn ctx(&self, project: ProjectId) -> Result<Arc<ProjectCtx>, Refused> {
        self.projects
            .lock()
            .expect("projects lock")
            .get(&project)
            .map(|p| Arc::clone(&p.ctx))
            .ok_or_else(|| Refused::NotFound(format!("no project {project}")))
    }

    fn ctl(&self, project: ProjectId) -> Option<mpsc::UnboundedSender<project::Ctl>> {
        self.projects
            .lock()
            .expect("projects lock")
            .get(&project)
            .map(|p| p.ctl.clone())
    }

    /// The brief: the project's first message, and the task it delegates to the CEO.
    pub async fn send_brief(
        &self,
        project: ProjectId,
        body: String,
        attachments: Vec<FileId>,
    ) -> Result<(MessageId, TaskId), Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::send_brief(&ctx, body, attachments)).await
    }

    /// The builder's message to the CEO, which opens a task for it when it has none from the
    /// builder.
    pub async fn send_chat(
        &self,
        project: ProjectId,
        body: String,
        attachments: Vec<FileId>,
        in_reply_to: Option<MessageId>,
    ) -> Result<MessageId, Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::send_chat(&ctx, body, attachments, in_reply_to)).await
    }

    pub async fn raise_task(
        &self,
        project: ProjectId,
        title: String,
        ask: String,
        part: Option<String>,
        attachments: Vec<FileId>,
    ) -> Result<TaskId, Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::raise_task(&ctx, title, ask, part, attachments)).await
    }

    /// The builder cancels a job. Any wait on it ends.
    pub async fn cancel_job(&self, project: ProjectId, job: JobId) -> Result<(), Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::cancel_job(&ctx, job)).await
    }

    /// A job the project's clock has fired: its `Fired` event is in the log, and this acts on
    /// it. Returns once the reaction is in the log.
    pub async fn job_fired(
        &self,
        project: ProjectId,
        job: Job<ProjectJob>,
        at: Timestamp,
    ) -> Result<(), Refused> {
        let ctl = self
            .ctl(project)
            .ok_or_else(|| Refused::NotFound(format!("no project {project}")))?;
        let (ack, done) = oneshot::channel();
        ctl.send(project::Ctl::Fired { job, at, ack })
            .map_err(|_| Refused::WrongState("the project's turn loop has stopped".into()))?;
        done.await
            .map_err(|_| Refused::WrongState("the project's turn loop has stopped".into()))
    }

    /// What the agent is waiting for, each with its deadline.
    pub fn waits(&self, project: ProjectId, agent: AgentId) -> Option<Vec<Wait>> {
        let ctx = self.ctx(project).ok()?;
        ctx.store
            .log
            .with_state(|s| s.agent(agent).map(|a| a.waits.values().cloned().collect()))
    }

    /// The start pack a driver was given, from its agent's transcript. Blocks on the store.
    pub fn start_pack(&self, project: ProjectId, driver: DriverId) -> Option<String> {
        let ctx = self.ctx(project).ok()?;
        pack::recorded(&ctx.store, driver)
    }

    /// Stops a driver the builder chose, and records it. Whatever turn it was in is lost and its
    /// inbox items go to the next driver. Returns once the driver's process is gone.
    pub async fn stop_driver(&self, project: ProjectId, driver: DriverId) -> Result<(), StopError> {
        let ctl = self.ctl(project).ok_or(StopError::NotFound)?;
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
