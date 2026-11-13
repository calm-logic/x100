//! Agents at work as an organisation: the CEO, leads and workers, delegation down the tree and
//! reports back up, the inbox, waits, jobs, and the start pack every driver starts from.
//!
//! - Each project has a turn loop ([`project`]) that gives an agent's waiting inbox items to its
//!   warm driver, or starts one, and a driver task per driver ([`driver`]) that records its turns.
//! - Agents act through the daemon's tools, which arrive at the [`Desk`] whatever carries them:
//!   MCP for the real harnesses ([`ToolServer`]), a callback for the fake one.
//!   [`tools`] decides each call from the folded state alone; the desk records it as an action,
//!   does the git work it needs, and appends what happened.
//! - Every append goes through one writer per project, which closes the waits its events end
//!   ([`settle`]), so the rule in `Wait::ended_by` has one home.
//!
//! - Every turn, a further turn on a warm driver included, runs on what the router in
//!   `x100-router` decides ([`route`]), with the room drivers take on the machine and on logins
//!   counted across every project ([`fleet`]).
//!
//! - Every driver runs in its agent's sandbox, reaching the outside only through its own route on
//!   the vault's proxy ([`sandbox`]), and everything a driver emits is redacted of credential
//!   values before it is recorded.
//!
//! - Every tool call, merge, request out through the proxy and command a permission hook stops
//!   is an action the gate of `x100-gate` allows or holds for the builder ([`gate`], [`admit`]),
//!   with the connected decision model asked about what no rule covers ([`classify`]), and the
//!   builder's decision is carried out and told to the agent ([`decide`]).

mod admit;
mod branches;
mod builder;
mod classify;
mod ctx;
mod decide;
mod desk;
mod driver;
mod egress;
mod fleet;
mod gate;
mod ids;
mod integrate;
mod jobs;
mod mcp;
pub mod pack;
mod processes;
mod project;
mod recover;
mod redact;
mod route;
mod sandbox;
mod settle;
mod spec;
mod store;
mod tools;
mod turns;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::{mpsc, oneshot};
use x100_core::action::Decision;
use x100_core::effort::EffortRange;
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{
    ActionId, AgentId, DriverId, FileId, HarnessId, JobId, MachineId, MessageId, ProjectId, TaskId,
};
use x100_core::job::{DaemonJob, Job, ProjectJob};
use x100_core::money::Money;
use x100_core::party::Actor;
use x100_core::settings::DaemonSettings;
use x100_core::time::{Clock, Timestamp};
use x100_core::wait::Wait;
use x100_drivers::Harness;
use x100_drivers::terminal::Size;
use x100_jobs::Scheduler;
use x100_router::Installed;
use x100_store::{DaemonLog, ProjectStore};

pub use builder::Refused;
pub use desk::{Desk, Refusal};
pub use egress::EgressEnd;
pub use ids::new_id;
pub use mcp::ToolServer;
pub use processes::{ProcessRegistry, Reaped};
pub use recover::recover;
pub use sandbox::Containment;

use ctx::ProjectCtx;
use store::blocking;
use x100_vault::Redactor;

/// Records that each agent's processes ended with its sandbox.
pub(crate) async fn processes_ended(
    ctx: &Arc<ProjectCtx>,
    ended: Vec<(AgentId, Vec<x100_core::process::ProcessName>)>,
) {
    let events: ctx::Events = ended
        .into_iter()
        .flat_map(|(agent, names)| {
            names.into_iter().map(move |name| {
                (
                    Actor::Daemon,
                    x100_core::event::ProjectEvent::ProcessStopped {
                        agent,
                        name,
                        end: x100_core::process::ProcessEnd::SandboxDestroyed,
                    },
                )
            })
        })
        .collect();
    if events.is_empty() {
        return;
    }
    let ctx = Arc::clone(ctx);
    if let Err(e) = blocking(move || ctx.writer().append(events).map(drop)).await {
        eprintln!("x100: the end of a sandbox's processes is not in the log: {e}");
    }
}

/// The daemon's settings as they are now, read whenever one is needed so a change reaches the
/// next decision.
pub type Settings = Arc<dyn Fn() -> DaemonSettings + Send + Sync>;

/// Everything the orchestrator is built from.
pub struct Config {
    pub clock: Arc<dyn Clock>,
    /// The harnesses this run can start, by the id the daemon's log gives each.
    pub harnesses: HashMap<HarnessId, Arc<dyn Harness>>,
    /// The machine those harnesses run on.
    pub machine: MachineId,
    /// The daemon's log: harnesses, machines, cool-offs and settings, which the router reads and
    /// a cool-off is written to.
    pub daemon: DaemonLog,
    /// The daemon log's job clock, told of each cool-off's end.
    pub daemon_jobs: Scheduler<DaemonState>,
    pub settings: Settings,
    /// Where each driver's process is written down, so a killed daemon's drivers can be found.
    pub processes: Arc<ProcessRegistry>,
    /// Where drivers' tool calls arrive. The harnesses are given it too, so it is made first.
    pub desk: Arc<Desk>,
    pub containment: Containment,
    /// Replaces every credential value it knows in what drivers emit, before it is recorded.
    pub redactor: Redactor,
    /// One turn at a time in each project, the agent whose oldest waiting item came first going
    /// next. Agents' processes run concurrently, so only this makes a log made on the fake
    /// harness the same on every run. The daemon runs turns side by side.
    pub serial: bool,
}

pub(crate) struct Shared {
    pub clock: Arc<dyn Clock>,
    /// Replaced whole when the builder asks for a rescan.
    pub harnesses: RwLock<HashMap<HarnessId, Arc<dyn Harness>>>,
    pub machine: MachineId,
    pub daemon: DaemonLog,
    pub daemon_jobs: Scheduler<DaemonState>,
    pub fleet: fleet::Fleet,
    pub settings: Settings,
    pub processes: Arc<ProcessRegistry>,
    pub desk: Arc<Desk>,
    pub sandboxes: Arc<sandbox::Sandboxes>,
    pub redactor: Redactor,
    pub serial: bool,
    /// Approved requests out that a request now going through is using, until its report.
    pub in_flight: Mutex<HashSet<ActionId>>,
}

impl Shared {
    pub fn harness(&self, id: HarnessId) -> Option<Arc<dyn Harness>> {
        self.harnesses
            .read()
            .expect("harnesses lock")
            .get(&id)
            .cloned()
    }

    /// What this daemon can start now.
    pub fn installed(&self) -> Installed {
        Installed {
            machine: self.machine,
            harnesses: self
                .harnesses
                .read()
                .expect("harnesses lock")
                .keys()
                .copied()
                .collect(),
        }
    }
}

/// Why a request for one driver, to stop it or to resize its terminal, was not carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DriverError {
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
        let sandboxes = Arc::new(sandbox::Sandboxes::new(
            config.containment,
            Arc::clone(&config.processes),
        ));
        config.desk.attach(Arc::clone(&sandboxes));
        Self {
            shared: Arc::new(Shared {
                clock: config.clock,
                harnesses: RwLock::new(config.harnesses),
                machine: config.machine,
                daemon: config.daemon,
                daemon_jobs: config.daemon_jobs,
                fleet: fleet::Fleet::new(),
                settings: config.settings,
                processes: config.processes,
                desk: config.desk,
                sandboxes,
                redactor: config.redactor,
                serial: config.serial,
                in_flight: Mutex::new(HashSet::new()),
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

    /// The builder's message to the CEO. Once the project is set up, it opens a task for the CEO
    /// when it has none from the builder; before, it is only a message.
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

    /// The builder renames a project that is set up.
    pub async fn rename(&self, project: ProjectId, name: String) -> Result<(), Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::rename(&ctx, name)).await
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
    pub async fn stop_driver(
        &self,
        project: ProjectId,
        driver: DriverId,
    ) -> Result<(), DriverError> {
        let ctl = self.ctl(project).ok_or(DriverError::NotFound)?;
        let (ack, done) = oneshot::channel();
        ctl.send(project::Ctl::Stop { driver, ack })
            .map_err(|_| DriverError::NotLive)?;
        match done.await {
            Ok(true) => Ok(()),
            _ => Err(DriverError::NotLive),
        }
    }

    /// A viewer's pane, in cells, for a live driver's terminal: its program redraws at that size,
    /// and the resize is recorded among its chunks. Returns once the terminal has been told.
    pub async fn resize_terminal(
        &self,
        project: ProjectId,
        driver: DriverId,
        size: Size,
    ) -> Result<(), DriverError> {
        let ctl = self.ctl(project).ok_or(DriverError::NotFound)?;
        let (ack, done) = oneshot::channel();
        ctl.send(project::Ctl::Resize { driver, size, ack })
            .map_err(|_| DriverError::NotLive)?;
        match done.await {
            Ok(true) => Ok(()),
            _ => Err(DriverError::NotLive),
        }
    }

    /// The builder sets a lead's budget. Refused if the leads' budgets would pass the project's
    /// total. The next decision for any of the lead's agents reads it.
    pub async fn set_budget(
        &self,
        project: ProjectId,
        lead: AgentId,
        budget: Money,
    ) -> Result<(), Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::set_budget(&ctx, lead, budget)).await
    }

    /// The builder sets an agent's effort range. A warm driver outside it is replaced at the
    /// agent's next turn.
    pub async fn set_effort(
        &self,
        project: ProjectId,
        agent: AgentId,
        range: EffortRange,
    ) -> Result<(), Refused> {
        let ctx = self.ctx(project)?;
        blocking(move || builder::set_effort(&ctx, agent, range)).await
    }

    /// Something in the daemon's log changed that could let an agent waiting for a driver find
    /// one: a machine's cap, a rescan of the harnesses. Every project tries its waiting agents
    /// again.
    pub fn reconsider(&self) {
        self.shared.fleet.changed();
    }

    /// The harnesses this run can start, and the programs a sandbox shows for them, after a
    /// rescan found them again.
    pub fn set_harnesses(
        &self,
        harnesses: HashMap<HarnessId, Arc<dyn Harness>>,
        programs: Vec<std::path::PathBuf>,
    ) {
        *self.shared.harnesses.write().expect("harnesses lock") = harnesses;
        self.shared.sandboxes.set_programs(programs);
        self.reconsider();
    }

    /// The builder approves or denies a held action. What it calls for is done before this
    /// returns, and the agent is told through its inbox.
    pub async fn decide(
        &self,
        project: ProjectId,
        action: ActionId,
        decision: Decision,
    ) -> Result<(), Refused> {
        let ctx = self.ctx(project)?;
        let shared = Arc::clone(&self.shared);
        blocking(move || decide::decide(&shared, &ctx, action, decision)).await
    }

    /// A job of the daemon's log fired: a cool-off ends, if that job is still what ends it, and
    /// agents waiting for a driver try again. Blocks on the store briefly.
    pub async fn daemon_job_fired(&self, job: Job<DaemonJob>) {
        let shared = Arc::clone(&self.shared);
        blocking(move || {
            let over = shared
                .daemon
                .with_state(|d| x100_router::cool_off_over(d, &job));
            if let Some(event) = over
                && let Err(e) = shared.daemon.append(Actor::Daemon, event)
            {
                eprintln!("x100: the end of a cool-off could not be recorded: {e}");
            }
        })
        .await;
        self.reconsider();
    }

    /// Stops every driver, recording a warm one as ended and a running one as failed, both
    /// because the daemon stopped, and returns when no driver process of ours is left and every
    /// driver's task, a crashed one's included, has finished, so none is still on record.
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
            // The loop returns once it has acknowledged; waiting for it leaves nothing behind.
            let _ = p.task.await;
            let ended = self.shared.sandboxes.destroy_project(p.ctx.id).await;
            processes_ended(&p.ctx, ended).await;
        }
    }
}
