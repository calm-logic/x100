//! An agent's transcript as clients see it. `x100-store` defines the same shape for what it
//! stores, because a client may not depend on the store; a test in the store keeps the two
//! serialisations identical. It belongs in `x100-core`, and moving it there would end the
//! duplication.

use serde::{Deserialize, Serialize};
use x100_core::id::DriverId;
use x100_core::money::Usage;
use x100_core::time::Timestamp;

/// An entry's place in one agent's transcript. Counts from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Position(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub at: Timestamp,
    pub driver: DriverId,
    pub item: TranscriptItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptItem {
    TurnStarted,
    Text {
        text: String,
    },
    ToolCall {
        call: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        call: String,
        output: String,
        is_error: bool,
    },
    Usage {
        usage: Usage,
    },
    TurnEnded {
        how: TurnEnd,
    },
    /// The first entry of a new driver. `from` is `None` for the agent's first.
    DriverChanged {
        from: Option<DriverId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEnd {
    Completed,
    Failed { detail: String },
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionedEntry {
    pub position: Position,
    pub entry: TranscriptEntry,
}
