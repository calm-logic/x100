//! The harness trait and what a driver is started with.

use std::ffi::OsString;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use x100_core::driver::{Failure, View};
use x100_core::effort::Effort;
use x100_core::harness::{HarnessKind, ModelName, ModelOffer, Support};
use x100_core::id::DriverId;
use x100_core::settings::DaemonSettings;

use crate::handle::DriverHandle;

/// An agent program x100 can run as a driver.
///
/// Every harness takes the start pack as its instructions, reaches the daemon's tools, streams what
/// it does and spends, ends a turn and stops when told. What [`Harness::supports`] reports false is
/// refused by the handle with [`crate::InputError::Unsupported`], never silently ignored.
#[async_trait]
pub trait Harness: Send + Sync {
    fn kind(&self) -> HarnessKind;
    /// Each model with the effort levels it runs at and the listed price of each.
    fn models(&self) -> Vec<ModelOffer>;
    fn supports(&self) -> Support;
    /// Whether its drivers run in a terminal, which interfaces open to show the driver, or
    /// headless, where the transcript is all there is.
    fn view(&self) -> View {
        View::Headless
    }
    /// Hosts the program reaches with the builder's own login, as its provider and to refresh
    /// the login. The egress proxy relays these untouched, TLS end to end, so x100 never sees
    /// the login in transit. Empty for a harness that runs on no login.
    fn login_hosts(&self) -> Vec<String> {
        Vec::new()
    }
    /// Where a harness run on an API key reads it, and the host the key is sent to, so the
    /// daemon can give it a vault reference the proxy swaps for the value there.
    fn api_key(&self) -> Option<ApiKey> {
        None
    }
    /// Starts the child process and returns once the harness has said it is ready, so a version it
    /// cannot work with is refused here rather than mid-task.
    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle, StartError>;
}

/// Everything one driver is started with.
#[derive(Debug, Clone)]
pub struct DriverSpec {
    pub driver: DriverId,
    pub model: ModelName,
    pub effort: Effort,
    /// The directory the driver works in: the agent's worktree.
    pub dir: PathBuf,
    /// The driver's own directory, for its home, configuration and sockets: made by the
    /// harness, removed when the driver ends. Writable where the driver runs, at the same path.
    pub state: PathBuf,
    /// How the child's command line is wrapped, so the sandbox can run it.
    pub wrap: CommandWrapper,
    /// How an address the harness serves on the daemon's side is made reachable where the
    /// driver runs, before the program that calls it is spawned.
    pub expose: Expose,
    /// Where the driver's `/tmp` is on the daemon's side, when it runs with a `/tmp` of its own,
    /// as in a sandbox. `None` when its `/tmp` is the daemon's.
    pub tmp: Option<PathBuf>,
    /// The driver's instructions, built by the daemon from its records.
    pub start_pack: String,
    /// Where this driver reaches the daemon's tools. `None` for a driver with no tools.
    pub tools: Option<ToolEndpoint>,
    /// Added to the child's clean environment, after everything the harness sets.
    pub env: Vec<(String, String)>,
    pub stall: StallPolicy,
    pub permissions: Permissions,
    /// Called with each child's process id as soon as it is spawned, before any handshake, so a
    /// daemon killed during the handshake has already written the process down. A terminal driver
    /// may call it more than once.
    pub on_spawn: SpawnHook,
    /// Where the builder's own login for this harness lives: the directory the harness keeps it
    /// in (`~/.claude`, `~/.codex`). Terminal harnesses run on it; headless ones take a key in
    /// [`DriverSpec::env`] and ignore it. x100 passes the path on and never opens the login.
    pub login: Option<PathBuf>,
}

/// This driver's own address for the daemon's tools over MCP (streamable HTTP), and the token
/// that tells the daemon which driver is calling.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolEndpoint {
    pub url: String,
    pub token: String,
}

impl fmt::Debug for ToolEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The token lets anyone act as this driver, so it stays out of logs.
        f.debug_struct("ToolEndpoint")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// Whether the harness asks the daemon before running a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permissions {
    /// Every call runs without asking. Calls to the daemon's own tools never ask: the daemon gates
    /// those itself.
    AllowAll,
    /// Calls the harness would ask about arrive as [`crate::DriverOutput::PermissionRequest`] and
    /// wait for an answer. Needs [`Support::permission_hook`].
    AskDaemon,
}

impl Permissions {
    /// Whether some calls are asked of the daemon.
    pub(crate) fn asks(self) -> bool {
        self != Permissions::AllowAll
    }
}

/// How long a driver may be silent before it counts as stalled.
///
/// Silence counts only while the driver owes output: starting, in a turn, or compacting, and not
/// while a permission request waits on the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StallPolicy {
    pub silent: Duration,
    /// The limit while a tool call the driver started has not returned: a long build or test run
    /// is silent and healthy.
    pub while_tool_runs: Duration,
    /// How often the supervisor reads the clock on its own. Tests set this long and call
    /// [`crate::DriverControl::check_stall`] after moving the fake clock.
    pub check_every: Duration,
}

impl StallPolicy {
    /// The daemon's stall timeout, six times as long while a tool runs.
    pub fn from_settings(settings: &DaemonSettings) -> Self {
        Self {
            silent: settings.stall_timeout,
            while_tool_runs: settings.stall_timeout.saturating_mul(6),
            check_every: Duration::from_secs(1),
        }
    }
}

/// A command line before it is run: the program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Rewrites a driver's command line, for example to run it inside a sandbox. The identity runs
/// it as it is.
#[derive(Clone)]
pub struct CommandWrapper(Arc<dyn Fn(CommandLine) -> CommandLine + Send + Sync>);

impl CommandWrapper {
    pub fn identity() -> Self {
        Self(Arc::new(|line| line))
    }

    pub fn new(wrap: impl Fn(CommandLine) -> CommandLine + Send + Sync + 'static) -> Self {
        Self(Arc::new(wrap))
    }

    pub fn apply(&self, line: CommandLine) -> CommandLine {
        (self.0)(line)
    }
}

impl Default for CommandWrapper {
    fn default() -> Self {
        Self::identity()
    }
}

impl fmt::Debug for CommandWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CommandWrapper")
    }
}

type Exposing = Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>;

/// Makes a loopback address on the daemon's side reachable at the same address where the driver
/// runs, as a sandbox's forwarder does. Without a sandbox there is nothing to do.
#[derive(Clone)]
pub struct Expose(Arc<dyn Fn(SocketAddr) -> Exposing + Send + Sync>);

impl Expose {
    /// For a driver that runs where the daemon does.
    pub fn nothing() -> Self {
        Self(Arc::new(|_| Box::pin(async { Ok(()) })))
    }

    pub fn new<F, Fut>(expose: F) -> Self
    where
        F: Fn(SocketAddr) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = std::io::Result<()>> + Send + 'static,
    {
        Self(Arc::new(move |addr| Box::pin(expose(addr))))
    }

    pub async fn expose(&self, addr: SocketAddr) -> std::io::Result<()> {
        (self.0)(addr).await
    }
}

impl Default for Expose {
    fn default() -> Self {
        Self::nothing()
    }
}

impl fmt::Debug for Expose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Expose")
    }
}

/// The variable a harness reads its provider key from, and the host it sends the key to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub env: String,
    pub host: String,
}

/// The host of a URL such as `https://api.example.com/v1`, lowercased.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// What a harness calls with a child's process id right after spawning it. It runs on the
/// starting task, so it should be quick.
#[derive(Clone)]
pub struct SpawnHook(Arc<dyn Fn(u32) + Send + Sync>);

impl SpawnHook {
    /// A hook that does nothing, for a caller that has no use for the pid.
    pub fn ignore() -> Self {
        Self(Arc::new(|_| {}))
    }

    pub fn new(hook: impl Fn(u32) + Send + Sync + 'static) -> Self {
        Self(Arc::new(hook))
    }

    pub(crate) fn call(&self, pid: u32) {
        (self.0)(pid)
    }
}

impl fmt::Debug for SpawnHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SpawnHook")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{harness:?} does not offer {model:?} at {effort:?} effort")]
    NotOffered {
        harness: HarnessKind,
        model: ModelName,
        effort: Effort,
    },
    #[error("this harness cannot {0}")]
    Unsupported(&'static str),
    #[error("could not prepare the driver: {0}")]
    Prepare(#[source] std::io::Error),
    #[error("could not start the child process: {0}")]
    Spawn(#[source] std::io::Error),
    /// The harness started but cannot be used, for example a version too old for x100.
    #[error("refused: {0}")]
    Refused(String),
    #[error("this harness runs on the builder's own login, and no login directory was given")]
    NoLogin,
    #[error("the driver failed while starting: {failure:?}; its last stderr: {stderr}")]
    Failed { failure: Failure, stderr: String },
}

/// Refuses a model and effort the harness does not list, before anything is spawned.
pub(crate) fn check_offered(
    kind: HarnessKind,
    models: &[ModelOffer],
    spec: &DriverSpec,
    support: Support,
) -> Result<(), StartError> {
    let offered = models
        .iter()
        .any(|m| m.model == spec.model && m.levels.iter().any(|(e, _)| *e == spec.effort));
    if !offered {
        return Err(StartError::NotOffered {
            harness: kind,
            model: spec.model.clone(),
            effort: spec.effort,
        });
    }
    if spec.permissions.asks() && !support.permission_hook {
        return Err(StartError::Unsupported("ask the daemon for permission"));
    }
    Ok(())
}

/// The variables a driver may inherit from the daemon's own environment by default. Nothing that
/// names a provider: keys reach a driver only through [`DriverSpec::env`].
pub const DEFAULT_INHERITED_ENV: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TERM", "TZ", "TMPDIR",
];

#[cfg(test)]
mod tests {
    #[test]
    fn a_urls_host_is_found_in_each_form() {
        for (url, host) in [
            ("https://api.example.com/v1", "api.example.com"),
            ("http://Provider.X100.test:8080", "provider.x100.test"),
            ("http://user:pw@host.test/x?y", "host.test"),
            ("http://[::1]:9/v1", "::1"),
            ("api.example.com", "api.example.com"),
        ] {
            assert_eq!(super::url_host(url).as_deref(), Some(host), "{url}");
        }
    }
}
