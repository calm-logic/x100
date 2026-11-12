//! What every connection shares: the daemon's log, the open projects, the orchestrator.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use x100_core::event::{DaemonEvent, ProjectEvent};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::ProjectId;
use x100_core::time::Clock;
use x100_jobs::Scheduler;
use x100_orch::{Desk, Orchestrator};
use x100_proto::{Cursor, ErrorCode, RpcError, Snapshot};
use x100_store::{DaemonLog, Follow, ProjectStore};
use x100_vault::Vault;

use crate::config::Config;

/// A project opened after a subscription began, for its subscriber to follow from the first event.
pub struct NewProject {
    pub id: ProjectId,
    pub store: ProjectStore,
}

#[derive(Default)]
struct Projects {
    open: BTreeMap<ProjectId, ProjectStore>,
    watchers: Vec<mpsc::UnboundedSender<NewProject>>,
}

pub struct Hub {
    pub config: Config,
    pub clock: Arc<dyn Clock>,
    pub log: DaemonLog,
    pub orch: Orchestrator,
    /// What harnesses found by a rescan reach the daemon's tools through.
    pub desk: Arc<Desk>,
    projects: Mutex<Projects>,
    /// The job clocks and what they feed, stopped with the daemon.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// One per project, so a job scheduled through it wakes the clock that fires it.
    schedulers: Mutex<BTreeMap<ProjectId, Scheduler<ProjectState>>>,
    /// The daemon log's job clock: cool-offs ending.
    daemon_jobs: Scheduler<DaemonState>,
    pub vault: Vault,
    /// Why no sandbox can be made on this machine, if none can.
    pub no_sandbox: Option<String>,
}

/// A subscription's logs, registered together with the snapshot or the cursor's events.
pub struct Registered {
    pub snapshot: Option<Snapshot>,
    pub daemon: Follow<DaemonEvent>,
    pub projects: Vec<(ProjectId, Follow<ProjectEvent>)>,
    /// Projects opened from now on.
    pub new_projects: mpsc::UnboundedReceiver<NewProject>,
}

impl Hub {
    pub fn new(
        config: Config,
        log: DaemonLog,
        orch: Orchestrator,
        desk: Arc<Desk>,
        daemon_jobs: Scheduler<DaemonState>,
        vault: Vault,
        no_sandbox: Option<String>,
    ) -> Self {
        Self {
            clock: Arc::clone(&config.clock),
            config,
            log,
            orch,
            desk,
            projects: Mutex::new(Projects::default()),
            tasks: Mutex::new(Vec::new()),
            schedulers: Mutex::new(BTreeMap::new()),
            daemon_jobs,
            vault,
            no_sandbox,
        }
    }

    /// Starts what runs for a project: its turn loop and its job clock. On a fake clock there is
    /// no job clock to run: `daemon.set_clock` fires what falls due.
    pub fn start_project(&self, id: ProjectId, store: ProjectStore) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = Scheduler::new(store.log.clone(), Arc::clone(&self.clock));
        self.orch.attach(id, store.clone(), scheduler.clone());
        self.schedulers
            .lock()
            .expect("schedulers lock")
            .insert(id, scheduler.clone());
        if self.config.fake_clock.is_some() {
            return;
        }
        let clock_task = tokio::spawn(async move {
            if let Err(e) = scheduler.run(tx).await {
                eprintln!("x100d: the job clock of project {id} stopped: {e}");
            }
        });
        let orch = self.orch.clone();
        let feed = tokio::spawn(async move {
            while let Some(fired) = rx.recv().await {
                let job = fired.job.id;
                if let Err(e) = orch.job_fired(id, fired.job, fired.at).await {
                    eprintln!("x100d: job {job} fired and could not be acted on: {e}");
                }
            }
        });
        self.tasks
            .lock()
            .expect("tasks lock")
            .extend([clock_task, feed]);
    }

    /// Runs the daemon log's job clock, whose jobs end cool-offs. On a fake clock
    /// `daemon.set_clock` fires them instead.
    pub fn start_daemon_jobs(&self) {
        if self.config.fake_clock.is_some() {
            return;
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = self.daemon_jobs.clone();
        let clock_task = tokio::spawn(async move {
            if let Err(e) = scheduler.run(tx).await {
                eprintln!("x100d: the daemon's job clock stopped: {e}");
            }
        });
        let orch = self.orch.clone();
        let feed = tokio::spawn(async move {
            while let Some(fired) = rx.recv().await {
                orch.daemon_job_fired(fired.job).await;
            }
        });
        self.tasks
            .lock()
            .expect("tasks lock")
            .extend([clock_task, feed]);
    }

    /// A task that runs until the daemon stops.
    pub fn keep(&self, task: JoinHandle<()>) {
        self.tasks.lock().expect("tasks lock").push(task);
    }

    /// Every project's job clock.
    pub fn schedulers(&self) -> Vec<(ProjectId, Scheduler<ProjectState>)> {
        self.schedulers
            .lock()
            .expect("schedulers lock")
            .iter()
            .map(|(id, s)| (*id, s.clone()))
            .collect()
    }

    pub fn daemon_jobs(&self) -> Scheduler<DaemonState> {
        self.daemon_jobs.clone()
    }

    pub fn scheduler(&self, id: ProjectId) -> Option<Scheduler<ProjectState>> {
        self.schedulers
            .lock()
            .expect("schedulers lock")
            .get(&id)
            .cloned()
    }

    pub fn stop_tasks(&self) {
        for t in self.tasks.lock().expect("tasks lock").drain(..) {
            t.abort();
        }
    }

    fn projects(&self) -> std::sync::MutexGuard<'_, Projects> {
        self.projects.lock().expect("projects lock")
    }

    pub fn project(&self, id: ProjectId) -> Option<ProjectStore> {
        self.projects().open.get(&id).cloned()
    }

    /// Makes a project known to the daemon, and tells every events subscription so it follows the
    /// project from its first event. Under the same lock as a subscription's registration, so a
    /// subscription either gets the project in its snapshot or is told of it, never both or neither.
    pub fn open_project(&self, id: ProjectId, store: ProjectStore) {
        let mut p = self.projects();
        p.open.insert(id, store.clone());
        p.watchers.retain(|w| {
            w.send(NewProject {
                id,
                store: store.clone(),
            })
            .is_ok()
        });
    }

    /// Registers an events subscription: a snapshot of every log and the stream after it, or the
    /// events after a cursor and the stream after those. Blocks on SQLite.
    pub fn register(&self, resume: Option<Cursor>) -> Result<Registered, RpcError> {
        let mut p = self.projects();
        let (new_tx, new_projects) = mpsc::unbounded_channel();
        let reject = |m: String| RpcError::new(ErrorCode::Rejected, m);
        let registered = match resume {
            None => {
                let (daemon_state, daemon) = self.log.snapshot_and_follow();
                let mut snapshot = Snapshot {
                    daemon: daemon_state,
                    projects: BTreeMap::new(),
                };
                let mut follows = Vec::new();
                for (id, store) in &p.open {
                    let (state, follow): (ProjectState, _) = store.log.snapshot_and_follow();
                    snapshot.projects.insert(*id, state);
                    follows.push((*id, follow));
                }
                Registered {
                    snapshot: Some(snapshot),
                    daemon,
                    projects: follows,
                    new_projects,
                }
            }
            Some(cursor) => {
                if let Some(id) = cursor.projects.keys().find(|id| !p.open.contains_key(id)) {
                    return Err(reject(format!(
                        "the cursor names project {id}, which is not open"
                    )));
                }
                if cursor.daemon > self.log.last() {
                    return Err(reject("the cursor is ahead of the daemon's log".into()));
                }
                let daemon = self
                    .log
                    .follow_after(cursor.daemon)
                    .map_err(|e| crate::methods::store_error(&e))?;
                let mut follows = Vec::new();
                for (id, store) in &p.open {
                    let after = cursor.projects.get(id).copied();
                    if after > store.log.last() {
                        return Err(reject(format!("the cursor is ahead of project {id}'s log")));
                    }
                    let follow = store
                        .log
                        .follow_after(after)
                        .map_err(|e| crate::methods::store_error(&e))?;
                    follows.push((*id, follow));
                }
                Registered {
                    snapshot: None,
                    daemon,
                    projects: follows,
                    new_projects,
                }
            }
        };
        p.watchers.push(new_tx);
        Ok(registered)
    }
}
