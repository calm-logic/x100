//! The daemon. It owns the data directory, opens the logs, runs each project's turn loop and job
//! clock, and serves the client protocol on a Unix socket. It is a library so that a test can
//! start one in its own process; the `x100d` program is a few lines around it.
//!
//! Clients come and go without stopping anything: a closed connection ends only its own
//! subscriptions. The daemon starts again from the store: drivers do not outlive it, so what the
//! log shows as live is settled first, and a process a killed daemon left behind is stopped.

mod config;
mod conn;
mod discover;
mod error;
mod hub;
mod methods;
mod unavailable;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use x100_core::time::Clock;
use x100_orch::{AllowAll, Desk, NoSandbox, Orchestrator, ProcessRegistry, ToolServer};
use x100_store::{ProjectLog, ProjectStore, open_daemon_log};

pub use config::{
    Config, DATA_DIR_ENV, FAKE_PROGRAM_ENV, LAG_AFTER, MAX_LINE, QUEUE_CAPACITY, SCRIPTS_ENV,
    default_data_dir,
};
pub use error::Error;
pub use unavailable::arrives_with;

use hub::Hub;

pub struct Daemon {
    hub: Arc<Hub>,
    /// The daemon's tools over MCP, for the real harnesses' drivers.
    tools: ToolServer,
    listener: UnixListener,
    socket: PathBuf,
    pid_file: PathBuf,
    stop: watch::Sender<bool>,
}

/// Asks a running daemon to stop, from anywhere.
#[derive(Clone)]
pub struct Stop(watch::Sender<bool>);

impl Stop {
    pub fn now(&self) {
        let _ = self.0.send(true);
    }
}

#[derive(Clone)]
pub struct Handle(Arc<Hub>);

impl Handle {
    pub fn project_log(&self, id: x100_core::id::ProjectId) -> Option<ProjectLog> {
        self.0.project(id).map(|s| s.log)
    }

    /// The scheduler whose clock fires the project's jobs.
    pub fn scheduler(
        &self,
        id: x100_core::id::ProjectId,
    ) -> Option<x100_jobs::Scheduler<x100_core::fold::ProjectState>> {
        self.0.scheduler(id)
    }
}

impl Daemon {
    /// Opens the data directory and gets ready to serve. Fails if another daemon holds it.
    pub async fn start(config: Config) -> Result<Daemon, Error> {
        std::fs::create_dir_all(&config.data_dir)?;
        let clock = Arc::clone(&config.clock);
        let log = open_daemon_log(&config.data_dir, Arc::clone(&clock)).map_err(|e| match e {
            x100_store::Error::Locked(_) => Error::AlreadyRunning(config.data_dir.clone()),
            other => Error::Store(other),
        })?;

        // Before anything starts: what a daemon that was killed left running must not act.
        let (processes, reaped) =
            ProcessRegistry::open_and_reap(config.data_dir.join("drivers.json"))?;
        for r in reaped {
            eprintln!(
                "x100d: stopped driver {}, left running by a daemon that died (pid {})",
                r.driver, r.pid
            );
        }

        let settings_log = log.clone();
        let settings: x100_orch::Settings =
            Arc::new(move || settings_log.with_state(|s| *s.settings()));
        // The harnesses reach the tools through the desk, so it exists before them, and so does
        // the address every driver is given.
        let desk = Desk::new(Arc::new(AllowAll), Arc::clone(&settings));
        let tools = ToolServer::start(Arc::clone(&desk)).await?;
        let found = discover::discover(&config, &log, desk.clone())?;
        // A rescan makes harnesses again, which reach the tools through the same desk.
        let rescan_tools = Arc::clone(&desk);
        // The daemon log's job clock: the orchestrator tells it of each cool-off it starts.
        let daemon_jobs = x100_jobs::Scheduler::new(log.clone(), Arc::clone(&clock));
        let orch = Orchestrator::new(x100_orch::Config {
            clock: Arc::clone(&clock),
            harnesses: found.harnesses,
            machine: found.machine,
            // No harness runs on a login yet: terminal drivers arrive with M3T.
            logins: Default::default(),
            daemon: log.clone(),
            daemon_jobs: daemon_jobs.clone(),
            settings,
            processes: Arc::new(processes),
            desk,
            sandbox: Arc::new(NoSandbox),
            // Turns run side by side, except where a fake clock asks for a repeatable run.
            serial: config.fake_clock.is_some(),
        });
        let hub = Arc::new(Hub::new(
            config.clone(),
            log,
            orch,
            rescan_tools,
            daemon_jobs,
        ));

        let projects: Vec<_> = hub
            .log
            .with_state(|d| d.projects().map(|(id, e)| (id, e.path.clone())).collect());
        for (id, path) in projects {
            let path = PathBuf::from(path);
            if !path.is_dir() {
                eprintln!("x100d: project {id} is gone from {}", path.display());
                continue;
            }
            let store = ProjectStore::open(&path, Arc::clone(&clock))?;
            // A project whose creation was cut short has nothing to settle or serve yet.
            if store.log.last().is_none() {
                continue;
            }
            x100_orch::recover(&store, &*clock)?;
            hub.open_project(id, store.clone());
            hub.start_project(id, store);
        }
        hub.start_daemon_jobs();

        // The log's lock says no other daemon is alive, so a socket file is a dead one's.
        let socket = config.socket_path();
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)?;
        let pid_file = config.data_dir.join("x100d.pid");
        std::fs::write(&pid_file, std::process::id().to_string())?;
        let (stop, _) = watch::channel(false);
        Ok(Daemon {
            hub,
            tools,
            listener,
            socket,
            pid_file,
            stop,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// Where the daemon serves its tools over MCP, on loopback: `http://127.0.0.1:<port>`.
    pub fn tools_url(&self) -> &str {
        self.tools.url()
    }

    pub fn stop_handle(&self) -> Stop {
        Stop(self.stop.clone())
    }

    /// A way into the running daemon's projects, for a test that must put something in a log
    /// that the protocol cannot yet.
    pub fn handle(&self) -> Handle {
        Handle(Arc::clone(&self.hub))
    }

    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.hub.clock)
    }

    /// Serves until [`Stop::now`], then stops every driver and leaves the data directory tidy.
    pub async fn run(self) {
        let mut conns = JoinSet::new();
        let mut stop = self.stop.subscribe();
        loop {
            tokio::select! {
                biased;
                _ = stop.changed() => break,
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        conns.spawn(conn::serve(Arc::clone(&self.hub), stream, self.stop.subscribe()));
                    }
                    Err(e) => eprintln!("x100d: could not accept a connection: {e}"),
                },
                Some(_) = conns.join_next(), if !conns.is_empty() => {}
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            while conns.join_next().await.is_some() {}
        })
        .await;
        conns.abort_all();
        self.hub.orch.shutdown().await;
        // No driver is left to call it.
        self.tools.stop().await;
        self.hub.stop_tasks();
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.pid_file);
    }
}
