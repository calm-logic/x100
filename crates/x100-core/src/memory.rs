//! Memories and skills. Their contents live in git repositories that only `x100-memory` touches;
//! the log records each change and each load.

use serde::{Deserialize, Serialize};

use crate::id::AgentId;

/// One memory note of one agent: a Markdown file with front matter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteName(pub String);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillName(pub String);

/// Where a skill lives: shared in the project, or one agent's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Shared,
    Agent(AgentId),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SkillRef {
    pub scope: SkillScope,
    pub name: SkillName,
}

/// The `version` field of a skill, raised on every committed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillVersion(pub u32);
