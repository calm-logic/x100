//! Creating, running in and destroying an isolated workspace for one agent.
//!
//! A [`Sandbox`] belongs to one agent and holds its git worktree. Every driver the agent has runs
//! in it through [`Sandbox::wrap`], one after another, and finds the files and the processes the
//! last one left. A process started with [`Sandbox::start_process`] belongs to the sandbox, not
//! to a driver, and runs until it is stopped or the sandbox is destroyed.
//!
//! Inside: the worktree read-write, a home of the sandbox's own, a private `/tmp`, the system
//! read-only, the programs the caller lists read-only, and the paths the caller names. Nothing
//! else of the host. No network: only the endpoints the caller maps appear, each at the same
//! loopback address it has on the host (see [`tunnel`]). Each command runs in a process
//! namespace of its own, so stopping it ends everything it started, a process that left its
//! session included; destroying the sandbox ends everything in it.
//!
//! Two backends implement [`Backend`]: [`Bubblewrap`] on Linux, and [`Container`] on Docker or
//! Podman, for macOS and as an option on Linux.

mod bwrap;
mod container;
mod git;
pub mod helper;
mod processes;
mod programs;
pub mod tunnel;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::process::Child;
use tokio::task::JoinHandle;

pub use bwrap::Bubblewrap;
pub use container::{Container, IMAGE, Runtime};
pub use git::git_paths;
pub use processes::{ProcessId, ProcessInfo};
pub use programs::program_paths;
pub use x100_drivers::{CommandLine, CommandWrapper};

use processes::Processes;

/// Makes sandboxes. Tests are written once against this and run on every backend.
#[async_trait]
pub trait Backend: Send + Sync {
    async fn create(&self, spec: SandboxSpec) -> Result<Sandbox, Error>;
}

/// What one agent's sandbox holds and reaches. Every path appears inside at the same path as on
/// the host, so a path a driver is given means the same thing on both sides.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    /// Unique on this machine: it names the container. Letters, digits, `-` and `_`.
    pub name: String,
    /// A host directory of the sandbox's own, made on create and removed on destroy: its home,
    /// its `/tmp`, and the logs of its processes.
    pub dir: PathBuf,
    /// The agent's worktree, read-write.
    pub worktree: PathBuf,
    /// Directories or files holding the programs a driver runs (`claude`, `codex`, node, cargo,
    /// rustup), read-only. Nothing else of the builder's home is visible.
    pub programs: Vec<PathBuf>,
    /// Further paths, each read-only or read-write: a terminal harness's login directory, the
    /// project's git directory (see [`git_paths`]).
    pub paths: Vec<Mount>,
    /// Loopback addresses on the host made reachable inside at the same address from the start,
    /// such as the daemon's tools endpoint. More can be mapped later with [`Sandbox::forward`].
    ///
    /// The sandbox sets no proxy variables: each driver's proxy URL carries a token of its own,
    /// so the caller maps the proxy's address and gives the URL in the command's environment. A
    /// sandbox given no proxy, as a lab's, has no way out at all.
    pub endpoints: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

/// A host path made visible inside at the same path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub path: PathBuf,
    pub access: Access,
}

impl Mount {
    pub fn read_only(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            access: Access::ReadOnly,
        }
    }

    pub fn read_write(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            access: Access::ReadWrite,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Spec(String),
    #[error("{what}: {source}")]
    Io {
        what: String,
        #[source]
        source: std::io::Error,
    },
    #[error("no process {0} in this sandbox")]
    NoProcess(ProcessId),
}

pub(crate) fn io(what: impl Into<String>) -> impl FnOnce(std::io::Error) -> Error {
    let what = what.into();
    move |source| Error::Io { what, source }
}

/// How a backend runs a command inside a sandbox it made.
pub(crate) trait Enter: Send + Sync {
    /// The command line that runs `line` inside, starting in the worktree. The caller gives it
    /// the environment the command should have.
    fn command(&self, line: CommandLine) -> CommandLine;
    /// Run on destroy, before the anchor is killed, where killing it is not enough.
    fn teardown(&self) -> Option<std::process::Command>;
}

/// One agent's sandbox, made by a [`Backend`].
pub struct Sandbox {
    spec: SandboxSpec,
    home: PathBuf,
    enter: Arc<dyn Enter>,
    /// The process that holds the sandbox's namespaces or container and carries its tunnel.
    anchor: u32,
    alive: Arc<AtomicBool>,
    anchor_task: Option<JoinHandle<()>>,
    /// Shared with each [`Forwarder`], which maps addresses while a driver starts.
    tunnel: Arc<tunnel::Host>,
    processes: Processes,
}

/// Where a backend puts things on the host, made on create.
pub(crate) struct Layout {
    pub home: PathBuf,
    pub tmp: PathBuf,
}

impl Layout {
    pub(crate) fn make(spec: &SandboxSpec) -> Result<Layout, Error> {
        check(spec)?;
        let layout = Layout {
            home: spec.dir.join("home"),
            tmp: spec.dir.join("tmp"),
        };
        for d in [&layout.home, &layout.tmp, &spec.dir.join("processes")] {
            std::fs::create_dir_all(d).map_err(io(format!("making {}", d.display())))?;
        }
        Ok(layout)
    }
}

fn check(spec: &SandboxSpec) -> Result<(), Error> {
    let name_ok = !spec.name.is_empty()
        && spec
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !name_ok {
        return Err(Error::Spec(format!("bad sandbox name {:?}", spec.name)));
    }
    let named = spec.paths.iter().map(|m| &m.path);
    for p in std::iter::once(&spec.worktree)
        .chain(&spec.programs)
        .chain(named)
        .chain(std::iter::once(&spec.dir))
    {
        if !p.is_absolute() {
            return Err(Error::Spec(format!("{} is not absolute", p.display())));
        }
    }
    for p in std::iter::once(&spec.worktree)
        .chain(&spec.programs)
        .chain(spec.paths.iter().map(|m| &m.path))
    {
        if !p.exists() {
            return Err(Error::Spec(format!("{} does not exist", p.display())));
        }
    }
    Ok(())
}

/// Every mount a command sees, parents before children, so a read-write path under a read-only
/// one stays writable.
pub(crate) fn mounts(spec: &SandboxSpec, layout: &Layout) -> Vec<Mount> {
    let mut all: Vec<Mount> = spec
        .programs
        .iter()
        .map(Mount::read_only)
        .chain(spec.paths.iter().cloned())
        .chain([
            Mount::read_write(&layout.home),
            Mount::read_write(&spec.worktree),
        ])
        .collect();
    all.sort_by_key(|m| m.path.components().count());
    all
}

/// The variables every command inside is given on top of its own.
pub(crate) fn sandbox_env() -> Vec<(String, String)> {
    vec![("TMPDIR".to_owned(), "/tmp".to_owned())]
}

impl Sandbox {
    /// Ties a started anchor to its tunnel and maps the spec's endpoints.
    pub(crate) async fn assemble(
        spec: SandboxSpec,
        layout: Layout,
        enter: Arc<dyn Enter>,
        mut anchor: Child,
        wait: Duration,
    ) -> Result<Sandbox, Error> {
        let pid = anchor.id().unwrap_or_default();
        let (Some(to), Some(from)) = (anchor.stdin.take(), anchor.stdout.take()) else {
            return Err(Error::Spec("the anchor has no pipes".into()));
        };
        let alive = Arc::new(AtomicBool::new(true));
        let anchor_task = tokio::spawn({
            let alive = Arc::clone(&alive);
            async move {
                let _ = anchor.wait().await;
                alive.store(false, Ordering::SeqCst);
            }
        });
        let sandbox = Sandbox {
            processes: Processes::new(spec.dir.join("processes")),
            home: layout.home,
            enter,
            anchor: pid,
            alive,
            anchor_task: Some(anchor_task),
            tunnel: match tunnel::Host::start(from, to, wait).await {
                Ok(t) => Arc::new(t),
                Err(e) => {
                    signal(pid, Signal::SIGKILL);
                    return Err(io("starting the sandbox")(e));
                }
            },
            spec,
        };
        for to in sandbox.spec.endpoints.clone() {
            if let Err(e) = sandbox.forward(to).await {
                sandbox.destroy().await?;
                return Err(e);
            }
        }
        Ok(sandbox)
    }

    pub fn worktree(&self) -> &Path {
        &self.spec.worktree
    }

    /// The sandbox's own home, read-write inside, at the same path as on the host.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Whether the sandbox is still there: false once destroyed, or if its anchor died.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// How a driver's command line is run inside. The driver gives the command its environment
    /// as usual; the sandbox adds the proxy's variables and `TMPDIR`. Once the sandbox is gone,
    /// a wrapped command fails to start instead of running outside it.
    pub fn wrap(&self) -> CommandWrapper {
        let enter = Arc::clone(&self.enter);
        let alive = Arc::clone(&self.alive);
        CommandWrapper::new(move |line| {
            if alive.load(Ordering::SeqCst) {
                enter.command(line)
            } else {
                CommandLine {
                    program: PathBuf::from("/nonexistent/x100-sandbox-is-gone"),
                    args: Vec::new(),
                }
            }
        })
    }

    /// Makes `to`, a loopback address on the host, reachable inside at the same address: for
    /// example the hook server a Claude Code terminal driver posts to.
    pub async fn forward(&self, to: SocketAddr) -> Result<(), Error> {
        self.forwarder().forward(to).await
    }

    /// What [`Sandbox::forward`] does, as a value a driver's start can hold without the sandbox.
    pub fn forwarder(&self) -> Forwarder {
        Forwarder(Arc::clone(&self.tunnel))
    }

    /// The process that holds the sandbox, the leader of its own process group. Killing that
    /// group ends everything in a bubblewrap sandbox; a container also needs
    /// [`Container::kill_leftovers`].
    pub fn anchor(&self) -> u32 {
        self.anchor
    }

    /// The sandbox's own directory on the host, removed on destroy.
    pub fn dir(&self) -> &Path {
        &self.spec.dir
    }

    /// The host directory that is `/tmp` inside, where a program may leave a socket it made.
    pub fn tmp(&self) -> PathBuf {
        self.spec.dir.join("tmp")
    }

    /// Starts a process that belongs to the sandbox and outlives any driver, with exactly `env`
    /// (plus `HOME`, the sandbox's own, if `env` has none). Its output goes to a log file.
    pub async fn start_process(
        &self,
        line: CommandLine,
        env: Vec<(String, String)>,
    ) -> Result<ProcessId, Error> {
        let mut env = env;
        if !env.iter().any(|(k, _)| k == "HOME") {
            env.push(("HOME".into(), self.home.display().to_string()));
        }
        let wrapped = self.wrap().apply(line.clone());
        self.processes
            .start(line, &wrapped, &env, &self.spec.worktree)
            .await
    }

    pub fn processes(&self) -> Vec<ProcessInfo> {
        self.processes.list()
    }

    /// Stops a process started with [`Sandbox::start_process`], with everything it started.
    pub async fn stop_process(&self, id: ProcessId) -> Result<(), Error> {
        self.processes.stop(id).await
    }

    /// Stops everything inside and removes the sandbox's own directory. The worktree stays: it
    /// belongs to the project.
    pub async fn destroy(mut self) -> Result<(), Error> {
        if let Some(mut teardown) = self.enter.teardown() {
            let _ = tokio::task::spawn_blocking(move || teardown.output()).await;
        }
        if self.alive.load(Ordering::SeqCst) {
            signal(self.anchor, Signal::SIGKILL);
        }
        if let Some(task) = self.anchor_task.take() {
            let _ = task.await;
        }
        self.processes.stop_all().await;
        let dir = self.spec.dir.clone();
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir))
            .await
            .map_err(|e| Error::Spec(e.to_string()))?
            .or_else(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            })
            .map_err(io(format!("removing {}", self.spec.dir.display())))
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // A sandbox dropped without `destroy` still ends: with its anchor gone, everything in
        // its process namespace or container is killed.
        if self.alive.load(Ordering::SeqCst) {
            signal(self.anchor, Signal::SIGKILL);
        }
        self.processes.kill_all();
    }
}

/// Maps loopback addresses of the host into one sandbox. Cheap to clone.
#[derive(Clone)]
pub struct Forwarder(Arc<tunnel::Host>);

impl Forwarder {
    pub async fn forward(&self, to: SocketAddr) -> Result<(), Error> {
        self.0
            .forward(to)
            .await
            .map_err(io(format!("mapping {to} into the sandbox")))
    }
}

pub(crate) fn signal(pid: u32, signal: Signal) {
    if let Ok(pid) = i32::try_from(pid) {
        let _ = kill(Pid::from_raw(pid), signal);
    }
}
