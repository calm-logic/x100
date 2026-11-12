//! Harnesses, the models they run, machines, cool-offs and one-off calls. These belong to the
//! daemon, not to a project, except one-off calls, which a project pays for.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::effort::Effort;
use crate::id::{ActionId, HarnessId, MachineId, OneOffCallId, TrialId};
use crate::money::Price;
use crate::time::Timestamp;

/// A provider's model, by the name its harness passes to the provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelName(pub String);

/// Which agent program a harness is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessKind {
    ClaudeCode,
    Codex,
    /// A harness around an LLM, built to the same interface.
    Custom {
        name: String,
    },
}

/// What a harness can do beyond the minimum every harness must. The daemon works without each of
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Support {
    /// Without it, every turn gets a new driver.
    pub further_turns: bool,
    /// Without it, a driver is retired where it would be compacted.
    pub compaction: bool,
    pub context_fill: bool,
    pub permission_hook: bool,
}

/// One model a harness runs, at each effort level it offers, with that level's price.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelOffer {
    pub model: ModelName,
    pub context_window: u64,
    /// How long the provider keeps this model's prompt cache. A driver of it is kept warm this
    /// long between turns, so a reused driver still finds its cache.
    pub cache_lifetime: Duration,
    pub levels: Vec<(Effort, Price)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Harness {
    pub id: HarnessId,
    pub kind: HarnessKind,
    pub support: Support,
    pub models: Vec<ModelOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Machine {
    pub id: MachineId,
    pub name: String,
    pub harnesses: Vec<HarnessId>,
    /// The most live drivers it may run at once.
    pub cap: u32,
}

/// A harness and model together: the unit the builder allows, and the unit that cools off.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HarnessModel {
    pub harness: HarnessId,
    pub model: ModelName,
}

/// A failure that looks like the provider's, which makes a harness and model cool off for every
/// project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFailure {
    Outage { detail: String },
    RateLimited { retry_after: Option<Duration> },
    KeyRefused,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CoolOff {
    pub of: HarnessModel,
    pub why: ProviderFailure,
    pub until: Timestamp,
}

/// Why a one-off call was made, which also says who pays for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OneOffPurpose {
    /// The gate's classifier, for an action the rules do not cover.
    Classify { action: ActionId },
    /// The lab's judge, scoring one run of a trial against a rubric.
    Judge { trial: TrialId },
}

/// A single turn with no tools, through the cheapest allowed harness and model at Low effort. It is
/// not a driver and belongs to no agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OneOffCall {
    pub id: OneOffCallId,
    pub purpose: OneOffPurpose,
    pub harness: HarnessId,
    pub model: ModelName,
}
