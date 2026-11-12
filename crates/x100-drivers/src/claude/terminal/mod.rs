//! Claude Code's own terminal program, unmodified, run in a pseudo-terminal on the builder's local
//! login.
//!
//! The login is the builder's Claude Code directory (`~/.claude`), reached as `$HOME/.claude` in a
//! home of the driver's own, so Claude Code reads it and rewrites it in place when the token
//! refreshes, and x100 never opens it. On macOS the login is in the Keychain under the default
//! name, which a driver reads because it sets no `CLAUDE_CONFIG_DIR`. Everything else of the
//! builder's setup is switched off: `--setting-sources ""` (their settings, hooks and plugins),
//! `--strict-mcp-config` (their MCP servers), `CLAUDE_CODE_DISABLE_CLAUDE_MDS` and
//! `CLAUDE_CODE_DISABLE_AUTO_MEMORY` (their CLAUDE.md and memory). The global state file
//! (`$HOME/.claude.json`, with the onboarding, theme and trust answers) is the driver's own.
//!
//! Claude Code still writes into the builder's directory what any session writes there: the
//! session file under `projects/`, `sessions/`, `session-env/`, `file-history/`,
//! `shell-snapshots/`, a prompt line in `history.jsonl`, and copies of the driver's own
//! `.claude.json` under `backups/`, where it keeps the five newest.
//! `cleanupPeriodDays` is set very high so that a driver never sweeps the builder's old sessions.

mod hooks;
mod protocol;
mod transcript;

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use x100_core::driver::View;
use x100_core::harness::{HarnessKind, ModelOffer, Support};
use x100_core::time::Clock;

use super::{TOOLS_SERVER, effort_flag, models};
use crate::handle::DriverHandle;
use crate::harness::{
    CommandLine, DEFAULT_INHERITED_ENV, DriverSpec, Harness, Permissions, StartError, check_offered,
};
use crate::supervisor::{self, Launch};
use crate::terminal::{Size, pty};
use crate::version::VersionPolicy;
use crate::wire::{Wire, remove_dir};

pub(crate) const VERSIONS: VersionPolicy = VersionPolicy {
    harness: "Claude Code",
    // The program status report on the terminal (OSC 7501) arrived in 2.1.295.
    minimum: "2.1.295",
    tested: "2.1.322",
};

/// The hooks a driver listens to. Each posts to the driver's own endpoint.
const HOOKS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Stop",
    "StopFailure",
    "PostCompact",
    "SessionEnd",
];

/// How long a held tool call may wait on the daemon before Claude Code gives up on the hook. A
/// builder may take a while to approve.
const PERMISSION_WAIT_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct ClaudeTerminalConfig {
    /// The `claude` executable.
    pub program: PathBuf,
    /// Where the provider is reached, if not Anthropic's own API: the fake provider in tests.
    pub base_url: Option<String>,
    /// Variables taken from the daemon's environment. Keys are never among them, and `HOME` is
    /// always the driver's own.
    pub inherit_env: Vec<String>,
    pub max_retries: u32,
    /// Each driver's home and settings are made under this one and removed when it ends.
    pub state_dir: PathBuf,
    pub models: Vec<ModelOffer>,
    /// The terminal's starting size.
    pub size: Size,
}

impl ClaudeTerminalConfig {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("claude"),
            base_url: None,
            inherit_env: DEFAULT_INHERITED_ENV
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            max_retries: 1,
            state_dir: state_dir.into(),
            models: models(),
            size: Size::default(),
        }
    }
}

/// Claude Code in a terminal, as a harness of its own: the router treats it apart from Claude
/// Code run headless, with its own models and prices.
pub struct ClaudeTerminal {
    config: ClaudeTerminalConfig,
    clock: Arc<dyn Clock>,
}

impl ClaudeTerminal {
    pub fn new(config: ClaudeTerminalConfig, clock: Arc<dyn Clock>) -> Self {
        Self { config, clock }
    }

    /// `<program> --version`, run as the driver will be.
    async fn version(
        &self,
        spec: &DriverSpec,
        env: &[(String, String)],
    ) -> Result<String, StartError> {
        let line = spec.wrap.apply(CommandLine {
            program: self.config.program.clone(),
            args: vec!["--version".into()],
        });
        let run = tokio::process::Command::new(&line.program)
            .args(&line.args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.dir)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(Duration::from_secs(30), run)
            .await
            .map_err(|_| StartError::Refused("`claude --version` did not answer".into()))?
            .map_err(StartError::Spawn)?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    fn settings(&self, port: u16, secret: &str, status_line: &str) -> serde_json::Value {
        let mut hooks = serde_json::Map::new();
        for event in HOOKS {
            let mut hook = json!({
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/hook/{event}"),
                "headers": { hooks::SECRET_HEADER: secret },
            });
            if *event == "PermissionRequest" {
                hook["timeout"] = json!(PERMISSION_WAIT_SECS);
            }
            hooks.insert((*event).to_owned(), json!([{ "hooks": [hook] }]));
        }
        json!({
            "hooks": hooks,
            // Claude Code tells only its status line how much of the login's usage windows is
            // used; the command records what it is given beside the driver's settings.
            "statusLine": { "type": "command", "command": status_line },
            "skipDangerousModePermissionPrompt": true,
            // Sessions older than this are swept from the login's directory, the builder's own
            // among them, so it is set past any the builder may have chosen.
            "cleanupPeriodDays": 36500,
        })
    }

    fn environment(&self, home: &Path) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self
            .config
            .inherit_env
            .iter()
            .filter(|k| *k != "HOME" && *k != "TERM")
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
            .collect();
        if !env.iter().any(|(k, _)| k == "LANG" || k == "LC_ALL") {
            env.push(("LANG".into(), "C.UTF-8".into()));
        }
        let set = [
            ("HOME", home.display().to_string()),
            ("TERM", "xterm-256color".into()),
            ("COLORTERM", "truecolor".into()),
            ("DISABLE_AUTOUPDATER", "1".into()),
            ("DISABLE_AUTO_COMPACT", "1".into()),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".into()),
            ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1".into()),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1".into()),
            // Background sessions and agent view share the login's directory, so without this a
            // driver lists, and can reach, the builder's own sessions.
            ("CLAUDE_CODE_DISABLE_AGENT_VIEW", "1".into()),
            // Not covered by the variable above: it fetches the official plugin marketplace.
            (
                "CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL",
                "1".into(),
            ),
            ("DISABLE_TELEMETRY", "1".into()),
            ("DISABLE_ERROR_REPORTING", "1".into()),
            (
                "CLAUDE_CODE_MAX_RETRIES",
                self.config.max_retries.to_string(),
            ),
        ];
        env.extend(set.into_iter().map(|(k, v)| (k.to_owned(), v)));
        if let Some(url) = &self.config.base_url {
            env.push(("ANTHROPIC_BASE_URL".into(), url.clone()));
        }
        env
    }
}

/// Records the status JSON Claude Code gives its status line, whole or not at all, and shows
/// nothing.
const STATUS_LINE: &str = "#!/bin/sh\n\
d=$(dirname \"$0\")\n\
cat > \"$d/status.json.part\" && mv \"$d/status.json.part\" \"$d/status.json\"\n";

/// A path as one word for `sh`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())
}

/// The driver's home: its own global state file, which answers the first-run questions, and the
/// builder's Claude Code directory as `.claude`.
fn make_home(home: &Path, login: &Path, worktree: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    std::os::unix::fs::symlink(login, home.join(".claude"))?;
    let mut projects = serde_json::Map::new();
    let trusted = json!({ "hasTrustDialogAccepted": true });
    projects.insert(worktree.display().to_string(), trusted.clone());
    if let Ok(real) = worktree.canonicalize() {
        projects.insert(real.display().to_string(), trusted);
    }
    let state = json!({
        "hasCompletedOnboarding": true,
        "theme": "dark",
        "projects": projects,
    });
    write_private(&home.join(".claude.json"), &state.to_string())
}

#[async_trait]
impl Harness for ClaudeTerminal {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCodeTerminal
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

    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle, StartError> {
        check_offered(self.kind(), &self.config.models, &spec, self.supports())?;
        let login = spec.login.clone().ok_or(StartError::NoLogin)?;
        let offer = self
            .config
            .models
            .iter()
            .find(|m| m.model == spec.model)
            .expect("checked as offered");
        let price = offer
            .levels
            .iter()
            .find(|(e, _)| *e == spec.effort)
            .map(|(_, p)| *p)
            .expect("checked as offered");
        let window = offer.context_window;

        let driver_dir = self.config.state_dir.join(spec.driver.to_string());
        let cleanup = Some(driver_dir.clone());
        let prepared = async {
            let home = driver_dir.join("home");
            let own = driver_dir.join("x100");
            std::fs::create_dir_all(&own).map_err(StartError::Prepare)?;
            make_home(&home, &login, &spec.dir).map_err(StartError::Prepare)?;
            let mut env = self.environment(&home);
            env.extend(spec.env.iter().cloned());
            let reported = self.version(&spec, &env).await?;
            // It answers `2.1.322 (Claude Code)`.
            let reported = reported
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            let warning = VERSIONS.check(&reported).map_err(StartError::Refused)?;
            Ok::<_, StartError>((home, own, env, reported, warning))
        }
        .await;
        let (_home, own, env, version, warning) = match prepared {
            Ok(p) => p,
            Err(e) => {
                remove_dir(cleanup.as_ref()).await;
                return Err(e);
            }
        };

        let mut wire = Wire::new();
        let secret = uuid::Uuid::new_v4().to_string();
        let session = uuid::Uuid::new_v4().to_string();
        let (port, server) = match hooks::serve(secret.clone(), wire.tx.clone()).await {
            Ok(s) => s,
            Err(e) => {
                remove_dir(cleanup.as_ref()).await;
                return Err(StartError::Prepare(e));
            }
        };
        wire.tasks.push(server);

        let write = |name: &str, content: &str| {
            let path = own.join(name);
            write_private(&path, content).map(|_| path.display().to_string())
        };
        let files = (|| {
            let status_line = own.join("status-line.sh");
            std::fs::write(&status_line, STATUS_LINE)?;
            std::fs::set_permissions(
                &status_line,
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            )?;
            let status_line = shell_quote(&status_line.display().to_string());
            let settings = write(
                "settings.json",
                &self.settings(port, &secret, &status_line).to_string(),
            )?;
            let pack = write("start-pack.md", &spec.start_pack)?;
            let mcp = match &spec.tools {
                Some(tools) => Some(write(
                    "mcp.json",
                    &json!({ "mcpServers": { TOOLS_SERVER: {
                        "type": "http",
                        "url": tools.url,
                        "headers": { "Authorization": format!("Bearer {}", tools.token) },
                    }}})
                    .to_string(),
                )?),
                None => None,
            };
            Ok::<_, std::io::Error>((settings, pack, mcp))
        })();
        let (settings, pack, mcp) = match files {
            Ok(f) => f,
            Err(e) => {
                wire.abandon().await;
                remove_dir(cleanup.as_ref()).await;
                return Err(StartError::Prepare(e));
            }
        };
        let mut args: Vec<String> = vec![
            "--model".into(),
            spec.model.0.clone(),
            "--effort".into(),
            effort_flag(spec.effort).into(),
            "--settings".into(),
            settings,
            "--setting-sources".into(),
            String::new(),
            "--strict-mcp-config".into(),
            "--append-system-prompt-file".into(),
            pack,
            "--session-id".into(),
            session.clone(),
            // Nobody answers a question asked on the terminal; agents ask through the daemon.
            "--disallowedTools".into(),
            "AskUserQuestion".into(),
        ];
        if let Some(mcp) = mcp {
            // The daemon gates its own tools, so the harness never asks about them.
            args.extend([
                "--mcp-config".into(),
                mcp,
                "--allowedTools".into(),
                format!("mcp__{TOOLS_SERVER}"),
            ]);
        }
        args.extend([
            "--permission-mode".into(),
            match spec.permissions {
                Permissions::AllowAll => "bypassPermissions",
                Permissions::AskDaemon => "manual",
                // Claude Code's own sandbox needs `socat` on Linux and is not relied on: its
                // commands are asked of the daemon through the permission hook instead.
                Permissions::Worktree => "acceptEdits",
            }
            .into(),
        ]);
        let line = spec.wrap.apply(CommandLine {
            program: self.config.program.clone(),
            args: args.into_iter().map(Into::into).collect(),
        });
        let (child, terminal) =
            match pty::spawn(&line, &env, &spec.dir, self.config.size, wire.tx.clone()) {
                Ok(spawned) => spawned,
                Err(e) => {
                    wire.abandon().await;
                    remove_dir(cleanup.as_ref()).await;
                    return Err(StartError::Spawn(e));
                }
            };
        spec.on_spawn.call(child.id().unwrap_or_default());
        let screen = terminal.screen();
        wire.terminal = Some(terminal);
        wire.add_child(child);
        let protocol = protocol::ClaudeTerminal::new(
            protocol::Setup {
                version,
                warnings: warning.into_iter().collect(),
                session,
                price,
                window,
                status: own.join("status.json"),
            },
            screen,
        );
        let launch = Launch {
            support: self.supports(),
            stall: spec.stall,
            permissions: spec.permissions,
            clock: Arc::clone(&self.clock),
            daemon_call: None,
            cleanup,
        };
        supervisor::launch(launch, Box::new(protocol), wire).await
    }
}
