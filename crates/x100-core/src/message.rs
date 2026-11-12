//! Messages between agents, and between the builder and the CEO, which is the chat.

use serde::{Deserialize, Serialize};

use crate::id::{AgentId, FileId, MessageId};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub route: Route,
    pub body: String,
    pub attachments: Vec<FileId>,
    pub in_reply_to: Option<MessageId>,
}

/// Who a message goes between. There is one chat per project, with the CEO: the builder messages
/// only the CEO, and only the CEO messages the builder, apart from the daemon's own notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    BuilderToCeo,
    CeoToBuilder,
    /// The daemon's notice in the chat, such as the project reaching its budget, which a CEO turn
    /// could not be paid for.
    DaemonToBuilder,
    Between {
        from: AgentId,
        to: AgentId,
    },
}
