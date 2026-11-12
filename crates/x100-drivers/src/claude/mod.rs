//! Claude Code, run headless on stream-json in both directions.
//!
//! Each driver gets a clean environment, its own `CLAUDE_CONFIG_DIR`, `--bare` (no hooks, plugins,
//! CLAUDE.md discovery or keychain), automatic updates and compaction off, retries set low, and an
//! MCP configuration for this run alone that points at the daemon's tools. The start pack is
//! appended to the system prompt through the `initialize` control request, so its size is not
//! limited by the command line.

mod stream;

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use x100_core::effort::Effort;
use x100_core::harness::{HarnessKind, ModelName, ModelOffer, Support};
use x100_core::money::{LongContext, Money, Price, Rates};
use x100_core::time::Clock;

use crate::handle::DriverHandle;
use crate::harness::{
    CommandLine, DEFAULT_INHERITED_ENV, DriverSpec, Harness, Permissions, StartError, check_offered,
};
use crate::supervisor::{self, Launch};
use crate::version::VersionPolicy;

pub(crate) const VERSIONS: VersionPolicy = VersionPolicy {
    harness: "Claude Code",
    // `--bare`, `--effort`, stream-json input and the permission prompt tool are all present
    // in 2.1; nothing older was tried.
    minimum: "2.1.0",
    tested: "2.1.322",
};

/// The MCP server name x100's tools appear under, so the model sees `mcp__x100__<tool>`.
pub const TOOLS_SERVER: &str = "x100";

#[derive(Debug, Clone)]
pub struct ClaudeCodeConfig {
    /// The `claude` executable.
    pub program: PathBuf,
    /// Where the provider is reached, if not Anthropic's own API. The fake provider in tests; the
    /// vault's proxy once M11 is built.
    pub base_url: Option<String>,
    /// Variables taken from the daemon's environment. Keys are never among them.
    pub inherit_env: Vec<String>,
    /// Claude Code's own retries of a failing request, kept low so the router moves work on.
    pub max_retries: u32,
    /// Each driver's configuration directory is made under this one and removed when it ends.
    pub state_dir: PathBuf,
    pub models: Vec<ModelOffer>,
}

impl ClaudeCodeConfig {
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
        }
    }
}

const fn micros(n: u64) -> Money {
    Money::from_micros(n)
}

/// Input, output and cache read per million tokens; cache writes are 1.25 times input for five
/// minutes and twice input for an hour.
fn rates(input: u64, output: u64, cache_read: u64) -> Rates {
    Rates {
        input: micros(input),
        output: micros(output),
        cache_read: micros(cache_read),
        cache_write_short: micros(input * 5 / 4),
        cache_write_long: micros(input * 2),
    }
}

/// The listed prices of the current Claude models, 2026-11. Claude Code reports its own cost, so
/// these serve the router's choice, not the ledger.
pub fn models() -> Vec<ModelOffer> {
    let offer = |model: &str, price: Price| ModelOffer {
        model: ModelName(model.to_owned()),
        context_window: 1_000_000,
        // Claude Code writes the five-minute cache.
        cache_lifetime: Duration::from_secs(5 * 60),
        levels: Effort::ALL.iter().map(|e| (*e, price)).collect(),
    };
    let flat = |r: Rates| Price {
        rates: r,
        long_context: None,
    };
    vec![
        offer(
            "claude-fable-5-2",
            flat(rates(8_000_000, 40_000_000, 200_000)),
        ),
        offer(
            "claude-opus-5-5",
            flat(rates(4_000_000, 20_000_000, 200_000)),
        ),
        offer(
            "claude-sonnet-5-5",
            flat(rates(2_000_000, 10_000_000, 100_000)),
        ),
        offer(
            "claude-haiku-5-5",
            Price {
                rates: rates(100_000, 500_000, 10_000),
                long_context: Some(LongContext {
                    above: 100_000,
                    rates: rates(500_000, 2_500_000, 50_000),
                }),
            },
        ),
    ]
}

pub(crate) fn effort_flag(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    }
}

pub struct ClaudeCode {
    config: ClaudeCodeConfig,
    clock: Arc<dyn Clock>,
}

impl ClaudeCode {
    pub fn new(config: ClaudeCodeConfig, clock: Arc<dyn Clock>) -> Self {
        Self { config, clock }
    }
}

fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())
}

#[async_trait]
impl Harness for ClaudeCode {
    fn kind(&self) -> HarnessKind {
        HarnessKind::ClaudeCode
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

    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle, StartError> {
        check_offered(self.kind(), &self.config.models, &spec, self.supports())?;
        let driver_dir = self.config.state_dir.join(spec.driver.to_string());
        let config_dir = driver_dir.join("claude-config");
        std::fs::create_dir_all(&config_dir).map_err(StartError::Prepare)?;

        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--bare",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--model",
            &spec.model.0,
            "--effort",
            effort_flag(spec.effort),
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        if let Some(tools) = &spec.tools {
            let mcp = serde_json::json!({
                "mcpServers": { TOOLS_SERVER: {
                    "type": "http",
                    "url": tools.url,
                    "headers": { "Authorization": format!("Bearer {}", tools.token) },
                }}
            });
            // A file only this user can read, so the token is not on the command line.
            let path = driver_dir.join("mcp.json");
            write_private(&path, &mcp.to_string()).map_err(StartError::Prepare)?;
            args.extend([
                "--mcp-config".to_owned(),
                path.display().to_string(),
                // The daemon gates its own tools, so the harness never asks about them.
                "--allowedTools".to_owned(),
                format!("mcp__{TOOLS_SERVER}"),
            ]);
        }
        match spec.permissions {
            Permissions::AskDaemon => {
                args.extend(["--permission-prompt-tool".into(), "stdio".into()])
            }
            Permissions::AllowAll => {
                args.extend(["--permission-mode".into(), "bypassPermissions".into()]);
            }
        }

        let mut env: Vec<(String, String)> = self
            .config
            .inherit_env
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
            .collect();
        let set = [
            ("CLAUDE_CONFIG_DIR", config_dir.display().to_string()),
            ("DISABLE_AUTOUPDATER", "1".into()),
            ("DISABLE_AUTO_COMPACT", "1".into()),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".into()),
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
        env.extend(spec.env.iter().cloned());

        let line = spec.wrap.apply(CommandLine {
            program: self.config.program.clone(),
            args: args.into_iter().map(Into::into).collect(),
        });
        let protocol = stream::ClaudeStream::new(spec.start_pack.clone(), spec.tools.is_some());
        let launch = Launch {
            line,
            env,
            dir: spec.dir.clone(),
            support: self.supports(),
            stall: spec.stall,
            permissions: spec.permissions,
            clock: Arc::clone(&self.clock),
            daemon_call: None,
            cleanup: Some(driver_dir),
        };
        supervisor::launch(launch, Box::new(protocol)).await
    }
}
