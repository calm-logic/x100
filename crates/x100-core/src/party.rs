//! Who does things: the builder, an agent, or the daemon itself.

use serde::{Deserialize, Serialize};

use crate::id::AgentId;

/// Who caused an event. The Log tab's "who".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    Builder,
    Agent(AgentId),
    Daemon,
}

/// The builder or an agent: someone who can raise or delegate a task, or answer a message. The
/// daemon is never one, since it acts on others' behalf: a job that delegates does so as the agent
/// that scheduled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Party {
    Builder,
    Agent(AgentId),
}

impl From<Party> for Actor {
    fn from(p: Party) -> Actor {
        match p {
            Party::Builder => Actor::Builder,
            Party::Agent(a) => Actor::Agent(a),
        }
    }
}
