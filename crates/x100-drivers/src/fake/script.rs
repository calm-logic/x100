//! The fake harness's script format.
//!
//! A script is a TOML file. It says who the fake poses as, the models and prices it offers, and
//! what it does on each turn. One binary plays any script, so two scripts make two providers.
//!
//! ```toml
//! [harness]
//! name = "fake-anthropic"          # becomes HarnessKind::Custom { name }
//! version = "1.0.0"                # reported at start (default "1.0.0")
//! # Anything left out is supported. A false one is refused by the driver's handle.
//! supports = { further_turns = true, compaction = true, context_fill = true, permission_hook = true }
//!
//! [[model]]
//! model = "fake-small"
//! context_window = 200000
//! cache_lifetime_secs = 300        # default 300
//! efforts = ["low", "medium"]
//! # Dollars per million tokens, at every effort listed. Unset classes cost nothing.
//! price = { input = 1.0, output = 5.0, cache_read = 0.1 }
//!
//! # Turns. Each turn input is played by the first entry not yet used whose `when` matches;
//! # `repeat = true` lets an entry be used again. No match fails the driver, saying why.
//! [[turn]]
//! when = { start_pack_contains = "ROLE: CEO", input_contains = "brief" }   # both optional
//! steps = [
//!   { text = "Forming the team." },
//!   # A daemon tool call: a core ToolCall in its serde form. The reply is saved as {{task}}.
//!   { call = { delegate = { to = { new_worker = { role = "dev", charter = "c" } }, task = { new = { title = "t", ask = "a", attachments = [] } }, effort = "low" } }, save = "task" },
//!   { text = "Delegated {{task}}; its id is {{task.id}} when the reply is JSON." },
//!   { write = "notes/plan.md", content = "plan" },     # a file in the driver's directory
//!   { run = "cargo test" },                            # a shell command in the directory
//!   { ask = "Bash", input = { command = "rm -rf build" } },   # a permission request
//!   { usage = { input = 1200, output = 300, cache_read = 0, cache_write_short = 0, cache_write_long = 0 } },
//!   { usage = { input = 10, output = 5, cost_micros = 42 } },  # a reported cost instead of a listed one
//!   { context = 15000 },                               # context fill, against the model's window
//!   { sleep_ms = 50 },
//!   # And the ways to fail, at any point:
//!   # { crash = 3 }                                    # the process exits with this code
//!   # { stall = true }                                 # it goes silent and never returns
//!   # { fail = "rate_limited", detail = "slow down" }  # outage | rate_limited | key_refused | run
//! ]
//!
//! [compaction]                     # optional: what a compaction does
//! context_after = 3000
//! steps = []
//! ```
//!
//! `{{name}}` in any string is replaced by a value saved earlier in the same driver; `{{input}}`
//! is the current turn's input. A turn ends by itself after its last step.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use x100_core::effort::Effort;
use x100_core::harness::{HarnessKind, ModelName, ModelOffer, Support};
use x100_core::money::{Money, Price, Rates};
use x100_core::tool::ToolCall;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub harness: Posing,
    #[serde(rename = "model")]
    pub models: Vec<ModelSpec>,
    #[serde(default, rename = "turn")]
    pub turns: Vec<TurnScript>,
    #[serde(default)]
    pub compaction: CompactionScript,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Posing {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub supports: Supports,
}

fn default_version() -> String {
    "1.0.0".into()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Supports {
    pub further_turns: bool,
    pub compaction: bool,
    pub context_fill: bool,
    pub permission_hook: bool,
}

impl Default for Supports {
    fn default() -> Self {
        Self {
            further_turns: true,
            compaction: true,
            context_fill: true,
            permission_hook: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    pub model: String,
    pub context_window: u64,
    #[serde(default = "default_cache_lifetime")]
    pub cache_lifetime_secs: u64,
    pub efforts: Vec<Effort>,
    pub price: PriceSpec,
}

fn default_cache_lifetime() -> u64 {
    300
}

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PriceSpec {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write_short: f64,
    pub cache_write_long: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnScript {
    #[serde(default)]
    pub when: When,
    #[serde(default)]
    pub repeat: bool,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    pub input_contains: Option<String>,
    pub start_pack_contains: Option<String>,
}

impl When {
    pub fn matches(&self, start_pack: &str, input: &str) -> bool {
        self.input_contains
            .as_deref()
            .is_none_or(|m| input.contains(m))
            && self
                .start_pack_contains
                .as_deref()
                .is_none_or(|m| start_pack.contains(m))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionScript {
    pub context_after: Option<u64>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UsageSpec {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write_short: u64,
    pub cache_write_long: u64,
    /// Set to report a cost, as Claude Code does. Unset, the cost is the tokens at the listed price.
    pub cost_micros: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailKind {
    Outage,
    RateLimited,
    KeyRefused,
    /// An error that is the run's, not the provider's.
    Run,
}

/// One thing a turn does. In the file, a table with one action key (see the module docs).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "RawStep")]
pub enum Step {
    Text(String),
    Call { call: Value, save: Option<String> },
    Write { path: String, content: String },
    Run(String),
    Ask { tool: String, input: Value },
    Usage(UsageSpec),
    Context(u64),
    Sleep(Duration),
    Crash(i32),
    Stall,
    Fail { kind: FailKind, detail: String },
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStep {
    text: Option<String>,
    call: Option<Value>,
    save: Option<String>,
    write: Option<String>,
    content: Option<String>,
    run: Option<String>,
    ask: Option<String>,
    input: Option<Value>,
    usage: Option<UsageSpec>,
    context: Option<u64>,
    sleep_ms: Option<u64>,
    crash: Option<i32>,
    stall: Option<bool>,
    fail: Option<FailKind>,
    detail: Option<String>,
}

impl TryFrom<RawStep> for Step {
    type Error = String;

    fn try_from(raw: RawStep) -> Result<Self, String> {
        let mut steps = Vec::new();
        if let Some(text) = raw.text {
            steps.push(Step::Text(text));
        }
        if let Some(call) = raw.call {
            steps.push(Step::Call {
                call,
                save: raw.save.clone(),
            });
        } else if raw.save.is_some() {
            return Err("`save` belongs with `call`".into());
        }
        if let Some(path) = raw.write {
            steps.push(Step::Write {
                path,
                content: raw.content.clone().unwrap_or_default(),
            });
        } else if raw.content.is_some() {
            return Err("`content` belongs with `write`".into());
        }
        if let Some(command) = raw.run {
            steps.push(Step::Run(command));
        }
        if let Some(tool) = raw.ask {
            steps.push(Step::Ask {
                tool,
                input: raw.input.clone().unwrap_or(Value::Null),
            });
        } else if raw.input.is_some() {
            return Err("`input` belongs with `ask`".into());
        }
        if let Some(usage) = raw.usage {
            steps.push(Step::Usage(usage));
        }
        if let Some(used) = raw.context {
            steps.push(Step::Context(used));
        }
        if let Some(ms) = raw.sleep_ms {
            steps.push(Step::Sleep(Duration::from_millis(ms)));
        }
        if let Some(code) = raw.crash {
            steps.push(Step::Crash(code));
        }
        if raw.stall == Some(true) {
            steps.push(Step::Stall);
        }
        if let Some(kind) = raw.fail {
            steps.push(Step::Fail {
                kind,
                detail: raw.detail.clone().unwrap_or_else(|| format!("{kind:?}")),
            });
        } else if raw.detail.is_some() {
            return Err("`detail` belongs with `fail`".into());
        }
        match steps.len() {
            1 => Ok(steps.remove(0)),
            0 => Err("a step needs one action: text, call, write, run, ask, usage, context, sleep_ms, crash, stall or fail".into()),
            _ => Err("a step has exactly one action".into()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("cannot read the script: {0}")]
    Read(#[from] std::io::Error),
    #[error("the script is not valid: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("the script is not valid: {0}")]
    Invalid(String),
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        let script: Script = toml::from_str(text)?;
        script.validate()?;
        Ok(script)
    }

    pub fn load(path: &Path) -> Result<Self, ScriptError> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    fn validate(&self) -> Result<(), ScriptError> {
        if self.models.is_empty() {
            return Err(ScriptError::Invalid(
                "at least one [[model]] is needed".into(),
            ));
        }
        let mut names = BTreeSet::new();
        for model in &self.models {
            if !names.insert(&model.model) {
                return Err(ScriptError::Invalid(format!(
                    "model {:?} is listed twice",
                    model.model
                )));
            }
            if model.efforts.is_empty() {
                return Err(ScriptError::Invalid(format!(
                    "model {:?} lists no efforts",
                    model.model
                )));
            }
        }
        let steps = self
            .turns
            .iter()
            .flat_map(|t| &t.steps)
            .chain(&self.compaction.steps);
        for step in steps {
            if let Step::Call { call, .. } = step {
                // A call with placeholders can only be checked once they are filled in.
                if !call.to_string().contains("{{") {
                    serde_json::from_value::<ToolCall>(call.clone()).map_err(|e| {
                        ScriptError::Invalid(format!("{call} is not a daemon tool call: {e}"))
                    })?;
                }
            }
        }
        Ok(())
    }

    pub fn kind(&self) -> HarnessKind {
        HarnessKind::Custom {
            name: self.harness.name.clone(),
        }
    }

    pub fn support(&self) -> Support {
        let s = self.harness.supports;
        Support {
            further_turns: s.further_turns,
            compaction: s.compaction,
            context_fill: s.context_fill,
            permission_hook: s.permission_hook,
        }
    }

    pub fn offers(&self) -> Vec<ModelOffer> {
        self.models
            .iter()
            .map(|m| ModelOffer {
                model: ModelName(m.model.clone()),
                context_window: m.context_window,
                cache_lifetime: Duration::from_secs(m.cache_lifetime_secs),
                levels: m.efforts.iter().map(|e| (*e, m.price.price())).collect(),
            })
            .collect()
    }
}

impl PriceSpec {
    fn price(&self) -> Price {
        let money =
            |dollars: f64| Money::from_micros((dollars * 1_000_000.0).round().max(0.0) as u64);
        Price {
            rates: Rates {
                input: money(self.input),
                output: money(self.output),
                cache_read: money(self.cache_read),
                cache_write_short: money(self.cache_write_short),
                cache_write_long: money(self.cache_write_long),
            },
            long_context: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        [harness]
        name = "fake"
        [[model]]
        model = "m"
        context_window = 1000
        efforts = ["low"]
        price = { input = 1.5 }
    "#;

    #[test]
    fn a_minimal_script_poses_as_a_harness_supporting_everything() {
        let script = Script::parse(MINIMAL).unwrap();
        assert_eq!(
            script.kind(),
            HarnessKind::Custom {
                name: "fake".into()
            }
        );
        assert!(script.support().compaction && script.support().permission_hook);
        let offer = &script.offers()[0];
        assert_eq!(offer.levels[0].1.rates.input, Money::from_micros(1_500_000));
    }

    #[test]
    fn a_step_has_exactly_one_action() {
        let two = format!("{MINIMAL}\n[[turn]]\nsteps = [{{ text = \"a\", run = \"b\" }}]");
        assert!(Script::parse(&two).is_err());
        let none = format!("{MINIMAL}\n[[turn]]\nsteps = [{{ }}]");
        assert!(Script::parse(&none).is_err());
        let stray = format!("{MINIMAL}\n[[turn]]\nsteps = [{{ text = \"a\", save = \"x\" }}]");
        assert!(Script::parse(&stray).is_err());
    }

    #[test]
    fn a_call_that_is_not_a_daemon_tool_call_is_refused_at_load() {
        let bad = format!("{MINIMAL}\n[[turn]]\nsteps = [{{ call = {{ fly = {{}} }} }}]");
        assert!(matches!(Script::parse(&bad), Err(ScriptError::Invalid(_))));
        let ok = format!(
            "{MINIMAL}\n[[turn]]\nsteps = [{{ call = {{ set_preview = {{ url = \"http://x\" }} }} }}]"
        );
        assert!(Script::parse(&ok).is_ok());
    }
}
