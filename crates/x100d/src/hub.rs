//! What every connection shares: the daemon's log, the open projects, the orchestrator.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use x100_core::event::{DaemonEvent, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::id::ProjectId;
use x100_core::job::ProjectJob;
use x100_core::time::Clock;
use x100_jobs::Scheduler;
use x100_orch::Orchestrator;
use x100_proto::{Cursor, ErrorCode, RpcError, Snapshot};
use x100_store::{DaemonLog, Follow, ProjectStore};

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
    projects: Mutex<Projects>,
    /// The job clocks and what they feed, stopped with the daemon.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// One per project, so a job scheduled through it wakes the clock that fires it.
    schedulers: Mutex<BTreeMap<ProjectId, Scheduler<ProjectState>>>,
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
    pub fn new(config: Config, log: DaemonLog, orch: Orchestrator) -> Self {
        Self {
            clock: Arc::clone(&config.clock),
            config,
            log,
            orch,
            projects: Mutex::new(Projects::default()),
            tasks: Mutex::new(Vec::new()),
            schedulers: Mutex::new(BTreeMap::new()),
        }
    }

    /// Starts what runs for a project: its turn loop and its job clock.
    pub fn start_project(&self, id: ProjectId, store: ProjectStore) {
        self.orch.attach(id, store.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = Scheduler::new(store.log.clone(), Arc::clone(&self.clock));
        self.schedulers
            .lock()
            .expect("schedulers lock")
            .insert(id, scheduler.clone());
        let clock = Arc::clone(&self.clock);
        let clock_task = tokio::spawn(async move {
            if let Err(e) = scheduler.run(tx).await {
                eprintln!("x100d: the job clock of project {id} stopped: {e}");
            }
        });
        let feed = tokio::spawn(async move {
            while let Some(fired) = rx.recv().await {
                // Delegating a task and the daemon's own routines arrive with M6 and M8; the
                // firing is in the log either way.
                if let ProjectJob::Wake { agent, note, .. } = fired.job.does {
                    let woken = x100_orch::deliver_job(&store, &*clock, agent, fired.job.id, note);
                    if let Err(e) = woken.await {
                        eprintln!("x100d: job {} could not wake {agent}: {e}", fired.job.id);
                    }
                }
            }
        });
        self.tasks
            .lock()
            .expect("tasks lock")
            .extend([clock_task, feed]);
    }

    /// Runs the daemon log's job clock. Its only job is a cool-off ending, which arrives with M8.
    pub fn start_daemon_jobs(&self) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = Scheduler::new(self.log.clone(), Arc::clone(&self.clock));
        let clock_task = tokio::spawn(async move {
            if let Err(e) = scheduler.run(tx).await {
                eprintln!("x100d: the daemon's job clock stopped: {e}");
            }
        });
        let feed = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        self.tasks
            .lock()
            .expect("tasks lock")
            .extend([clock_task, feed]);
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
