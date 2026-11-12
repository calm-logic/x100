//! Codex's own terminal program attached to Codex's app-server, on the builder's local login.
//!
//! The driver runs `codex app-server --listen unix://SOCKET` and drives it with the same JSON-RPC
//! as the headless harness, carried in WebSocket frames on the socket. Once the thread is started
//! and stored, it runs `codex --remote unix://SOCKET resume THREAD` in a pseudo-terminal, which
//! draws every turn the daemon starts, for the builder to watch.
//!
//! The login: each driver has a `CODEX_HOME` of its own with x100's configuration, as the
//! headless driver has, and `auth.json` there is a link to the one in the builder's Codex
//! directory. Codex 0.167 follows the link when it rewrites the login on a token refresh, so the
//! builder's file stays current and x100 never opens it. Nothing else of the builder's Codex
//! directory is read or written. `HOME` is the driver's own too, so skills in the builder's home
//! are not loaded either.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use x100_core::driver::View;
use x100_core::harness::{HarnessKind, ModelOffer, Support};
use x100_core::time::Clock;

use super::{Provider, TOKEN_VAR, config_toml, models, rpc};
use crate::handle::DriverHandle;
use crate::harness::{
    CommandLine, DEFAULT_INHERITED_ENV, DriverSpec, Harness, StartError, check_offered, url_host,
};
use crate::process;
use crate::supervisor::{self, Launch};
use crate::terminal::{Size, pty};
use crate::wire::{Incoming, Wire, remove_dir, wait_for};

/// How long the app-server has to open its socket.
const SOCKET_WAIT: Duration = Duration::from_secs(30);

/// ChatGPT, OpenAI's sign-in and API: where Codex refreshes its login and sends its requests
/// on that login.
const LOGIN_HOSTS: &[&str] = &[
    "chatgpt.com",
    "ab.chatgpt.com",
    "auth.openai.com",
    "api.openai.com",
];

#[derive(Debug, Clone)]
pub struct CodexTerminalConfig {
    /// The `codex` executable.
    pub program: PathBuf,
    /// Where OpenAI is reached, if not at its own address: the fake provider in tests.
    pub base_url: Option<String>,
    /// Variables taken from the daemon's environment. `HOME` is always the driver's own.
    pub inherit_env: Vec<String>,
    /// Codex's own retries of a failing request and of a broken stream, kept low.
    pub max_retries: u32,
    pub models: Vec<ModelOffer>,
    pub size: Size,
}

impl Default for CodexTerminalConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexTerminalConfig {
    pub fn new() -> Self {
        Self {
            program: PathBuf::from("codex"),
            base_url: None,
            inherit_env: DEFAULT_INHERITED_ENV
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            max_retries: 1,
            models: models(),
            size: Size::default(),
        }
    }
}

/// Codex in a terminal, as a harness of its own beside Codex run headless.
pub struct CodexTerminal {
    config: CodexTerminalConfig,
    clock: Arc<dyn Clock>,
}

impl CodexTerminal {
    pub fn new(config: CodexTerminalConfig, clock: Arc<dyn Clock>) -> Self {
        Self { config, clock }
    }

    fn environment(
        &self,
        home: &Path,
        codex_home: &Path,
        spec: &DriverSpec,
    ) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self
            .config
            .inherit_env
            .iter()
            .filter(|k| *k != "HOME" && *k != "TERM")
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
            .collect();
        env.push(("HOME".into(), home.display().to_string()));
        env.push(("CODEX_HOME".into(), codex_home.display().to_string()));
        env.push(("TERM".into(), "xterm-256color".into()));
        env.push(("COLORTERM".into(), "truecolor".into()));
        if let Some(tools) = &spec.tools {
            env.push((TOKEN_VAR.into(), tools.token.clone()));
        }
        env.extend(spec.env.iter().cloned());
        env
    }

    /// The driver's directories, its configuration and the link to the login.
    fn prepare(&self, spec: &DriverSpec, dir: &Path, login: &Path) -> std::io::Result<()> {
        let codex_home = dir.join("codex-home");
        std::fs::create_dir_all(&codex_home)?;
        std::fs::create_dir_all(dir.join("home"))?;
        let socket_dir = dir.join("socket");
        std::fs::create_dir_all(&socket_dir)?;
        // Codex refuses a socket in a directory others could write to.
        std::fs::set_permissions(
            &socket_dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )?;
        let provider = Provider::Login {
            base_url: self.config.base_url.as_deref(),
            retries: self.config.max_retries,
        };
        std::fs::write(codex_home.join("config.toml"), config_toml(&provider, spec))?;
        std::os::unix::fs::symlink(login.join("auth.json"), codex_home.join("auth.json"))
    }
}

/// Where the socket Codex made is on the daemon's side. Codex 0.167 makes it under a fixed
/// `/tmp/codex-daemon-<uid>/` and links to it from the path asked for, so in a driver with a
/// `/tmp` of its own the link names a path that is under `tmp` here.
fn socket_outside(socket: &Path, tmp: Option<&Path>) -> std::io::Result<PathBuf> {
    let target = match std::fs::read_link(socket) {
        Ok(t) if t.is_relative() => socket.parent().unwrap_or(Path::new("/")).join(t),
        Ok(t) => t,
        Err(_) => return Ok(socket.to_path_buf()),
    };
    match (tmp, target.strip_prefix("/tmp")) {
        (Some(tmp), Ok(rest)) => Ok(tmp.join(rest)),
        _ => Ok(target),
    }
}

/// Connects to a Unix socket whatever the length of its path. A socket's path is limited to about
/// a hundred bytes, and one in a sandbox's `/tmp` seen from the daemon's side is longer; on Linux
/// the socket is reached through a descriptor of its directory, whose path is short.
async fn connect_unix(socket: &Path) -> std::io::Result<UnixStream> {
    const SHORT_ENOUGH: usize = 100;
    if socket.as_os_str().len() < SHORT_ENOUGH || !cfg!(target_os = "linux") {
        return UnixStream::connect(socket).await;
    }
    let (Some(dir), Some(name)) = (socket.parent(), socket.file_name()) else {
        return UnixStream::connect(socket).await;
    };
    let dir = std::fs::File::open(dir)?;
    let short = Path::new("/proc/self/fd")
        .join(std::os::fd::AsRawFd::as_raw_fd(&dir).to_string())
        .join(name);
    // The directory stays open until the connection is made.
    let stream = UnixStream::connect(&short).await;
    drop(dir);
    stream
}

/// Carries JSON-RPC lines in WebSocket text frames, both ways.
async fn connect(socket: &Path, wire: &mut Wire) -> std::io::Result<()> {
    let stream = connect_unix(socket).await?;
    let (ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .map_err(std::io::Error::other)?;
    let (mut sink, mut frames) = ws.split();
    let (lines_tx, mut lines_rx) = mpsc::unbounded_channel::<String>();
    let tx = wire.tx.clone();
    wire.lines = Some(lines_tx);
    wire.tasks.push(tokio::spawn(async move {
        let broken = loop {
            tokio::select! {
                frame = frames.next() => match frame {
                    Some(Ok(Message::Text(text))) => {
                        let _ = tx.send(Incoming::Line(text.as_str().to_owned()));
                    }
                    Some(Ok(Message::Close(_))) | None => break false,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break true,
                },
                line = lines_rx.recv() => match line {
                    Some(line) => {
                        if sink.send(Message::text(line)).await.is_err() {
                            break true;
                        }
                    }
                    None => {
                        let _ = sink.close().await;
                        return;
                    }
                },
            }
        };
        let _ = tx.send(Incoming::Closed { broken });
    }));
    Ok(())
}

#[async_trait]
impl Harness for CodexTerminal {
    fn kind(&self) -> HarnessKind {
        HarnessKind::CodexTerminal
    }

    fn models(&self) -> Vec<ModelOffer> {
        self.config.models.clone()
    }

    fn supports(&self) -> Support {
        Support {
            further_turns: true,
            compaction: true,
            context_fill: true,
            permission_hook: true,
        }
    }

    fn view(&self) -> View {
        View::Terminal
    }

    fn login_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = LOGIN_HOSTS.iter().map(|h| (*h).to_owned()).collect();
        hosts.extend(self.config.base_url.as_deref().and_then(url_host));
        hosts
    }

    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle, StartError> {
        check_offered(self.kind(), &self.config.models, &spec, self.supports())?;
        let login = spec.login.clone().ok_or(StartError::NoLogin)?;
        let price = self
            .config
            .models
            .iter()
            .find(|m| m.model == spec.model)
            .and_then(|m| m.levels.iter().find(|(e, _)| *e == spec.effort))
            .map(|(_, p)| *p)
            .expect("checked as offered");
        let dir = spec.state.clone();
        let socket = dir.join("socket").join("app.sock");
        let cleanup = Some(dir.clone());
        if let Err(e) = self.prepare(&spec, &dir, &login) {
            remove_dir(cleanup.as_ref()).await;
            return Err(StartError::Prepare(e));
        }
        let env = self.environment(&dir.join("home"), &dir.join("codex-home"), &spec);
        let address = format!("unix://{}", socket.display());

        let server = spec.wrap.apply(CommandLine {
            program: self.config.program.clone(),
            args: ["app-server", "--listen", &address]
                .iter()
                .map(Into::into)
                .collect(),
        });
        let mut child = match process::spawn(&server, &env, &spec.dir) {
            Ok(child) => child,
            Err(e) => {
                remove_dir(cleanup.as_ref()).await;
                return Err(StartError::Spawn(e));
            }
        };
        spec.on_spawn.call(child.id().unwrap_or_default());
        let mut wire = Wire::new();
        wire.tail(child.stdout.take().expect("stdout is piped"));
        wire.tail(child.stderr.take().expect("stderr is piped"));
        let opened = wait_for(SOCKET_WAIT, || {
            let exited = child.try_wait().ok().flatten().is_some();
            (exited || socket.symlink_metadata().is_ok()).then_some(!exited)
        })
        .await;
        wire.add_child(child);
        let connected = match opened {
            Some(true) => match socket_outside(&socket, spec.tmp.as_deref()) {
                Ok(real) => connect(&real, &mut wire).await,
                Err(e) => Err(e),
            },
            _ => Err(std::io::Error::other(format!(
                "codex app-server did not open {}",
                socket.display()
            ))),
        };
        if let Err(e) = connected {
            let stderr = wire
                .stderr
                .lock()
                .expect("tail")
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            wire.abandon().await;
            remove_dir(cleanup.as_ref()).await;
            return Err(StartError::Refused(format!(
                "{e}; its output: {}",
                stderr.join("\n")
            )));
        }
        let tx = wire.tx.clone();
        let protocol = rpc::CodexRpc::new(&spec, price, true);
        let launch = Launch {
            support: self.supports(),
            stall: spec.stall,
            permissions: spec.permissions,
            clock: Arc::clone(&self.clock),
            daemon_call: None,
            cleanup,
        };
        let handle = supervisor::launch(launch, Box::new(protocol), wire).await?;

        let thread = handle.info().session.clone().unwrap_or_default();
        let viewer = spec.wrap.apply(CommandLine {
            program: self.config.program.clone(),
            args: ["--remote", &address, "resume", &thread]
                .iter()
                .map(Into::into)
                .collect(),
        });
        match pty::spawn(&viewer, &env, &spec.dir, self.config.size, tx) {
            Ok((child, terminal)) => {
                spec.on_spawn.call(child.id().unwrap_or_default());
                if handle.control().attach(child, terminal).await.is_ok() {
                    return Ok(handle);
                }
                Err(StartError::Refused(
                    "the driver ended before its terminal attached".into(),
                ))
            }
            Err(e) => {
                handle.stop().await;
                Err(StartError::Spawn(e))
            }
        }
    }
}
