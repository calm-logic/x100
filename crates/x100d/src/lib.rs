//! The daemon. It owns the data directory, opens the logs, runs each project's turn loop and job
//! clock, and serves the client protocol on a Unix socket. It is a library so that a test can
//! start one in its own process; the `x100d` program is a few lines around it.
//!
//! Clients come and go without stopping anything: a closed connection ends only its own
//! subscriptions. The daemon starts again from the store: drivers do not outlive it, so what the
//! log shows as live is settled first, and a process a killed daemon left behind is stopped.
//!
//! Every driver runs in its agent's sandbox, and reaches the outside only through the vault's
//! proxy. Where no sandbox can be made, no driver starts unless the builder turns on the daemon's
//! `run_without_sandbox` setting, and every harness says why it is passed over.

mod config;
mod conn;
mod contain;
mod defaults;
mod discover;
mod error;
mod hub;
mod methods;
mod unavailable;
mod vault;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use x100_core::time::Clock;
use x100_orch::{Containment, Desk, Orchestrator, ProcessRegistry, ToolServer};
use x100_store::{ProjectLog, ProjectStore, open_daemon_log};

pub use config::{
    CLAUDE_BASE_URL_ENV, CLAUDE_LOGIN_ENV, CODEX_BASE_URL_ENV, CODEX_LOGIN_ENV, Config,
    DATA_DIR_ENV, FAKE_PROGRAM_ENV, LAG_AFTER, MAX_LINE, NO_KEYCHAIN_ENV, PROJECTS_ROOT_ENV,
    PROXY_RESOLVE_ENV, PROXY_UPSTREAM_CA_ENV, QUEUE_CAPACITY, SANDBOX_ENV, SANDBOX_HELPER_ENV,
    SCRIPTS_ENV, SandboxKind, default_data_dir,
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
        let root = config.projects_root.clone();
        // The orchestrator reads the projects root from the settings, so the daemon's default
        // fills a setting the builder has not made.
        let settings: x100_orch::Settings = Arc::new(move || {
            let mut s = settings_log.with_state(|s| s.settings().clone());
            s.projects_root.get_or_insert_with(|| root.clone());
            s
        });
        let vault_config = config.clone();
        let opened = tokio::task::spawn_blocking(move || vault::open(&vault_config))
            .await
            .expect("opening the vault panicked")?;
        let (vault_handle, check_log) = (opened.vault.clone(), log.clone());
        tokio::task::spawn_blocking(move || vault::reconcile(&vault_handle, &check_log))
            .await
            .expect("checking the vault panicked")?;
        let backend = contain::backend(&config).await;
        let no_sandbox = backend.as_ref().err().cloned();
        if let Some(why) = &no_sandbox {
            eprintln!("x100d: no sandbox can be made on this machine: {why}");
        }

        // The harnesses reach the tools through the desk, so it exists before them, and so does
        // the address every driver is given.
        let desk = Desk::new(Arc::clone(&settings), opened.vault.redactor());
        let tools = ToolServer::start(Arc::clone(&desk)).await?;
        let found = discover::discover(&config, &log, desk.clone(), no_sandbox.as_deref())?;
        // A rescan makes harnesses again, which reach the tools through the same desk.
        let rescan_tools = Arc::clone(&desk);
        // The daemon log's job clock: the orchestrator tells it of each cool-off it starts.
        let daemon_jobs = x100_jobs::Scheduler::new(log.clone(), Arc::clone(&clock));
        let orch = Orchestrator::new(x100_orch::Config {
            clock: Arc::clone(&clock),
            harnesses: found.harnesses,
            machine: found.machine,
            daemon: log.clone(),
            daemon_jobs: daemon_jobs.clone(),
            settings,
            processes: Arc::new(processes),
            desk,
            containment: Containment {
                backend,
                dir: contain::dir(&config),
                tag: contain::tag(&config),
                programs: found.programs,
                proxy: Some((opened.proxy.clone(), opened.certificate.clone())),
            },
            redactor: opened.vault.redactor(),
            // Turns run side by side, except where a fake clock asks for a repeatable run.
            serial: config.fake_clock.is_some(),
        });
        // Before any driver starts: until then the proxy lets nothing out.
        opened.proxy.admit_with(orch.admission());
        let reports = vault::drain(opened.reports, orch.clone());
        let hub = Arc::new(Hub::new(
            config.clone(),
            log,
            orch,
            rescan_tools,
            daemon_jobs,
            opened.vault,
            no_sandbox,
        ));
        hub.keep(reports);

        let projects: Vec<_> = hub
            .log
            .with_state(|d| d.projects().map(|(id, _)| id).collect());
        for id in projects {
            let store = ProjectStore::open(
                &x100_store::project_dir(&config.data_dir, id),
                Arc::clone(&clock),
            )?;
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
