//! Git references the log points at: checkpoints, merges, and commits to memory and skill
//! repositories.

use serde::{Deserialize, Serialize};

/// A git commit, by its full hash.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommitId(pub String);
