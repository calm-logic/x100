//! Files the project holds outside the code, stored by content hash.

use serde::{Deserialize, Serialize};

use crate::id::{FileId, MessageId};
use crate::party::Party;

/// A hash of a file's bytes, which is also its name in the project's file store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileRef {
    pub id: FileId,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub hash: ContentHash,
    pub added_by: Party,
    /// Set when it came attached to a chat message.
    pub attached_to: Option<MessageId>,
}
