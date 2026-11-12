//! An agent's transcript: what its drivers said and did, in x100's own format so no provider's
//! transcript is relied on. The store keeps it and the client protocol carries it, so the type
//! lives here, where both may depend on it.

use serde::{Deserialize, Serialize};

use crate::id::DriverId;
use crate::money::Usage;
use crate::time::Timestamp;

/// An entry's place in one agent's transcript. Counts from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pos(pub u64);

/// One thing in an agent's transcript, and the driver it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub at: Timestamp,
    pub driver: DriverId,
    pub item: Item,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    /// The first entry of each driver: the start pack it was started from, as it was given. It is
    /// the only record of it, so a driver's start can be read back whatever happened since.
    StartPack {
        text: String,
    },
    TurnStarted,
    /// Text the model wrote.
    Text {
        text: String,
    },
    ToolCall {
        /// Pairs the call with its result and its live output.
        call: String,
        name: String,
        input: serde_json::Value,
    },
    /// A piece of a running tool's terminal output, as it arrives. The Team tab shows these live;
    /// the `ToolResult` that follows carries the whole output.
    ToolOutput {
        call: String,
        chunk: String,
    },
    ToolResult {
        call: String,
        output: String,
        is_error: bool,
    },
    /// What the turn used, as the harness reported it.
    Usage {
        usage: Usage,
    },
    TurnEnded {
        how: TurnEnd,
    },
    /// Follows a new driver's start pack: the agent's work carries on in the same transcript, with
    /// the change marked. `from` is `None` for the agent's first driver.
    DriverChanged {
        from: Option<DriverId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEnd {
    Completed,
    Failed {
        detail: String,
    },
    /// Stopped while running, by the daemon or the builder.
    Interrupted,
}
