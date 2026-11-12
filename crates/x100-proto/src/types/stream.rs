//! Following the logs and transcripts, and reading them back a page at a time.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent, Seq};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{AgentId, ProjectId};

use crate::transcript::{Position, PositionedEntry, TranscriptEntry};

/// Names one log: the daemon's, or one project's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogId {
    Daemon,
    Project(ProjectId),
}

/// Identifies one subscription on one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubscriptionId(pub u64);

/// How far a client has read each log. A log it lacks is read from its first event.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub daemon: Option<Seq>,
    pub projects: BTreeMap<ProjectId, Seq>,
}

/// The daemon's state and every project's, each as of its own sequence number, which `seq()` on the
/// state reports.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub daemon: DaemonState,
    pub projects: BTreeMap<ProjectId, ProjectState>,
}

impl Snapshot {
    /// Where to resume from after this snapshot.
    pub fn cursor(&self) -> Cursor {
        Cursor {
            daemon: self.daemon.seq(),
            projects: self
                .projects
                .iter()
                .filter_map(|(id, s)| s.seq().map(|q| (*id, q)))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribe {
    /// Where the client got to, if it is reconnecting. Without it, the result carries a snapshot.
    pub resume: Option<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribed {
    pub subscription: SubscriptionId,
    /// Present when `resume` was absent. Every event notified on this subscription is after it.
    pub snapshot: Option<Snapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unsubscribe {
    pub subscription: SubscriptionId,
}

/// Which events a page of a log holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum Page {
    /// Newest first, starting below `before` (from the latest for `None`). The Log tab.
    NewestFirst { before: Option<Seq>, limit: u32 },
    /// Oldest first, starting above `after` (from the first for `None`).
    OldestFirst { after: Option<Seq>, limit: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadLog {
    pub log: LogId,
    pub page: Page,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "log", rename_all = "snake_case")]
pub enum LogPage {
    Daemon { events: Vec<Envelope<DaemonEvent>> },
    Project { events: Vec<Envelope<ProjectEvent>> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadTranscript {
    pub project: ProjectId,
    pub agent: AgentId,
    pub after: Option<Position>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub entries: Vec<PositionedEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeTranscript {
    pub project: ProjectId,
    pub agent: AgentId,
    /// The entries after this position are notified first, then new ones as they are written.
    pub after: Option<Position>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSubscribed {
    pub subscription: SubscriptionId,
}

/// Why a daemon ended a subscription on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedBecause {
    /// The client read too slowly and fell too far behind. Subscribe again with a cursor.
    Lagged,
    Shutdown,
}

/// One event of one log, notified on an events subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "log", rename_all = "snake_case")]
pub enum EventNotice {
    Daemon {
        subscription: SubscriptionId,
        envelope: Envelope<DaemonEvent>,
    },
    Project {
        subscription: SubscriptionId,
        project: ProjectId,
        envelope: Envelope<ProjectEvent>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptNotice {
    pub subscription: SubscriptionId,
    pub position: Position,
    pub entry: TranscriptEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionClosed {
    pub subscription: SubscriptionId,
    pub because: ClosedBecause,
}
