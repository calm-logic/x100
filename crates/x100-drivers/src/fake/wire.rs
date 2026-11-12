//! The JSON lines between the fake harness's child and its driver.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use x100_core::effort::Effort;

use super::script::{FailKind, UsageSpec};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FromFake {
    Ready {
        version: String,
    },
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
        /// A daemon tool call, which the driver makes for the fake and answers with
        /// [`ToFake::DaemonResult`].
        daemon: bool,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
    },
    Permission {
        id: String,
        tool: String,
        input: Value,
    },
    Usage(UsageSpec),
    Context {
        used: u64,
    },
    TurnEnded {
        interrupted: bool,
    },
    Compacted,
    Error {
        detail: String,
        kind: FailKind,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToFake {
    Start {
        start_pack: String,
        model: String,
        effort: Effort,
    },
    Turn {
        input: String,
    },
    Compact,
    Interrupt,
    DaemonResult {
        id: String,
        ok: bool,
        text: String,
    },
    PermissionAnswer {
        id: String,
        allow: bool,
        reason: String,
    },
}
