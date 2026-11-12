//! What a driver emits.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use x100_core::driver::{ContextFill, Failure};
use x100_core::ledger::CostBasis;
use x100_core::money::{Money, Usage};
use x100_core::terminal::Chunk;

/// One thing a driver did, in the order it did it. The stream ends after [`DriverOutput::Failed`]
/// or once the driver is stopped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverOutput {
    /// A block of the model's text.
    Text(String),
    /// A tool call, by the harness's own name for the tool. Calls to the daemon's tools appear here
    /// too, as the harness names them.
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
    },
    /// The harness wants to run a tool call and waits for [`crate::DriverControl::answer`].
    PermissionRequest(PermissionRequest),
    /// What the turn, or a compaction, cost. Comes before the [`DriverOutput::TurnEnded`] or
    /// [`DriverOutput::Compacted`] it belongs to.
    Spent(Spent),
    Context(ContextFill),
    TurnEnded(TurnEnd),
    Compacted,
    /// Something the daemon should record and a person may need to act on, such as a harness
    /// version x100 was not tested with.
    Warning(String),
    /// A piece of the driver's terminal, as it happens: bytes its program wrote or a resize, the
    /// starting size first. Only drivers whose harness runs in a terminal emit these.
    Terminal(Chunk),
    /// The driver has failed and its process group has been stopped. Always the last output.
    Failed(Failure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEnd {
    Completed,
    /// The turn was ended early by [`crate::DriverControl::interrupt`].
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spent {
    pub usage: Usage,
    pub cost: Money,
    /// Reported by the harness, or the reported tokens at the model's listed price.
    pub basis: CostBasis,
}

/// The harness's own id for a pending permission request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermissionId(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: PermissionId,
    /// The harness's name for the tool, such as `Bash` or `command`.
    pub tool: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    /// The reason is shown to the model, for example that the action is held for the builder.
    Deny {
        reason: String,
    },
}
