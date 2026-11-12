//! Codex, run as `codex app-server` on stdio: JSON-RPC 2.0 as JSON lines.
//!
//! Each driver gets a clean environment and its own `CODEX_HOME` holding a `config.toml` x100
//! writes: the provider as a custom one (base URL and key variable), Codex's own sandbox off,
//! approvals as the spec asks, update checks, automatic compaction, AGENTS.md loading and the
//! features that reach other services off, retries set low, and the daemon's tools as an MCP
//! server whose bearer token is read from the environment. Codex reports tokens only, so the cost
//! is computed here from the model's listed price.

mod rpc;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use x100_core::effort::Effort;
use x100_core::harness::{HarnessKind, ModelName, ModelOffer, Support};
use x100_core::money::{LongContext, Money, Price, Rates};
use x100_core::time::Clock;

use crate::handle::DriverHandle;
use crate::harness::{
    CommandLine, DEFAULT_INHERITED_ENV, DriverSpec, Harness, StartError, check_offered,
};
use crate::supervisor::{self, Launch};
use crate::version::VersionPolicy;

pub(crate) const VERSIONS: VersionPolicy = VersionPolicy {
    harness: "Codex",
    // The app-server v2 methods x100 uses (thread/start, turn/start, thread/compact/start,
    // turn/interrupt) are those of 0.167; nothing older was tried.
    minimum: "0.167.0",
    tested: "0.167.1",
};

/// The variable the daemon's tools token is passed in, named in the MCP configuration.
const TOKEN_VAR: &str = "X100_TOOLS_TOKEN";

#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// The `codex` executable.
    pub program: PathBuf,
    /// The Responses API base URL.
    pub base_url: String,
    /// The environment variable holding the key, set through [`DriverSpec::env`].
    pub key_env: String,
    pub inherit_env: Vec<String>,
    /// Codex's own retries of a failing request and of a broken stream, kept low.
    pub max_retries: u32,
    /// Each driver's `CODEX_HOME` is made under this one and removed when it ends.
    pub state_dir: PathBuf,
    pub models: Vec<ModelOffer>,
}

impl CodexConfig {
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("codex"),
            base_url: "https://api.openai.com/v1".into(),
            key_env: "OPENAI_API_KEY".into(),
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

/// Cached input at a tenth of input, cache writes at the input price, and twice everything for a
/// turn whose input passes 272K tokens.
fn price(input: u64, output: u64) -> Price {
    let rates = |input: u64, output: u64| Rates {
        input: Money::from_micros(input),
        output: Money::from_micros(output),
        cache_read: Money::from_micros(input / 10),
        cache_write_short: Money::from_micros(input),
        cache_write_long: Money::from_micros(input),
    };
    Price {
        rates: rates(input, output),
        long_context: Some(LongContext {
            above: 272_000,
            rates: rates(input * 2, output * 2),
        }),
    }
}

/// The listed prices of the current Codex models, 2026-11. The window is what gpt-6-luna
/// reported as `modelContextWindow`; the others were not seen and are assumed to match.
pub fn models() -> Vec<ModelOffer> {
    let offer = |model: &str, price: Price| ModelOffer {
        model: ModelName(model.to_owned()),
        context_window: 258_400,
        cache_lifetime: Duration::from_secs(5 * 60),
        levels: Effort::ALL.iter().map(|e| (*e, price)).collect(),
    };
    vec![
        offer("gpt-6-astra", price(10_000_000, 50_000_000)),
        offer("gpt-6.1-sol", price(2_000_000, 10_000_000)),
        offer("gpt-6.1-terra", price(1_000_000, 5_000_000)),
        offer("gpt-6-luna", price(100_000, 500_000)),
    ]
}

pub(crate) fn effort_name(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Max => "max",
    }
}

/// A TOML basic string. JSON's string escapes are all valid in TOML.
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).expect("a string serializes")
}

/// Features that reach services other than the provider, or that make Codex start agents and
/// tools of its own.
const FEATURES_OFF: &[&str] = &[
    "apps",
    "plugins",
    "remote_plugin",
    "plugin_discovery_prefetch",
    "plugin_sharing",
    "multi_agent",
    "browser_use",
    "computer_use",
    "image_generation",
    "tool_suggest",
    "skill_mcp_dependency_install",
    "unbounded_connection_retries",
    "api_key_model_discovery",
    "realtime_conversation",
    "goals",
];

/// The approval policy is set on `thread/start`, not here: 0.167.1 refuses `untrusted` in the file
/// but takes it on the thread.
fn config_toml(config: &CodexConfig, spec: &DriverSpec) -> String {
    let mut toml = format!(
        "model = {model}\n\
         model_provider = \"x100\"\n\
         sandbox_mode = \"danger-full-access\"\n\
         check_for_update_on_startup = false\n\
         project_doc_max_bytes = 0\n\
         model_auto_compact_token_limit = 1000000000000\n\
         \n[features]\n",
        model = toml_str(&spec.model.0),
    );
    for feature in FEATURES_OFF {
        toml.push_str(&format!("{feature} = false\n"));
    }
    toml.push_str(&format!(
        "\n[analytics]\nenabled = false\n\
         \n[model_providers.x100]\n\
         name = \"x100\"\n\
         base_url = {base_url}\n\
         env_key = {env_key}\n\
         wire_api = \"responses\"\n\
         request_max_retries = {retries}\n\
         stream_max_retries = {retries}\n",
        base_url = toml_str(&config.base_url),
        env_key = toml_str(&config.key_env),
        retries = config.max_retries,
    ));
    if let Some(tools) = &spec.tools {
        toml.push_str(&format!(
            "\n[mcp_servers.x100]\nurl = {url}\nbearer_token_env_var = \"{TOKEN_VAR}\"\n",
            url = toml_str(&tools.url),
        ));
    }
    toml
}

pub struct Codex {
    config: CodexConfig,
    clock: Arc<dyn Clock>,
}

impl Codex {
    pub fn new(config: CodexConfig, clock: Arc<dyn Clock>) -> Self {
        Self { config, clock }
    }
}

#[async_trait]
impl Harness for Codex {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
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
        let price = self
            .config
            .models
            .iter()
            .find(|m| m.model == spec.model)
            .and_then(|m| m.levels.iter().find(|(e, _)| *e == spec.effort))
            .map(|(_, p)| *p)
            .expect("checked as offered");
        let driver_dir = self.config.state_dir.join(spec.driver.to_string());
        let home = driver_dir.join("codex-home");
        std::fs::create_dir_all(&home).map_err(StartError::Prepare)?;
        std::fs::write(home.join("config.toml"), config_toml(&self.config, &spec))
            .map_err(StartError::Prepare)?;

        let mut env: Vec<(String, String)> = self
            .config
            .inherit_env
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
            .collect();
        env.push(("CODEX_HOME".into(), home.display().to_string()));
        if let Some(tools) = &spec.tools {
            env.push((TOKEN_VAR.into(), tools.token.clone()));
        }
        env.extend(spec.env.iter().cloned());

        let line = spec.wrap.apply(CommandLine {
            program: self.config.program.clone(),
            args: ["app-server", "--listen", "stdio://"]
                .iter()
                .map(Into::into)
                .collect(),
        });
        let protocol = rpc::CodexRpc::new(&spec, price);
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
            on_spawn: spec.on_spawn.clone(),
        };
        supervisor::launch(launch, Box::new(protocol)).await
    }
}
