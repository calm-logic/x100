//! Processes the daemon runs in an agent's sandbox, such as a dev server.
//!
//! A harness kills the background commands it started when it stops, so a process that must
//! outlive a driver is started by the daemon through `start_process`. It belongs to the sandbox, not
//! to a driver, and keeps running through a handoff.

use serde::{Deserialize, Serialize};

use crate::id::AgentId;

/// The name an agent gives a process, unique among its running ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProcessName(pub String);

/// A process as it starts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Process {
    /// The agent whose sandbox it runs in.
    pub agent: AgentId,
    pub name: ProcessName,
    pub command: String,
}

/// Why a process stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessEnd {
    /// An agent called `stop_process`.
    Stopped,
    /// It exited by itself. No code when a signal ended it.
    Exited { code: Option<i32> },
    /// Its sandbox was destroyed.
    SandboxDestroyed,
}
