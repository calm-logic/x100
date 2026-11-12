//! Specs: the BRD, PRD and ADD, stored as Markdown in the project and written from the log.

use serde::{Deserialize, Serialize};

use crate::id::AgentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecKind {
    /// Business requirements, written by the CEO.
    Brd,
    /// Product requirements, written by the Product lead or the CEO.
    Prd,
    /// Architecture design, written by the Tech lead or the CEO.
    Add,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecStatus {
    Draft,
    Approved,
}

/// One revision of one spec. The log holds every revision, so the Markdown files can be written
/// from it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpecRevision {
    pub kind: SpecKind,
    pub author: AgentId,
    /// 1 for the first revision, counted by the daemon.
    pub revision: u32,
    pub status: SpecStatus,
    pub body: String,
}
