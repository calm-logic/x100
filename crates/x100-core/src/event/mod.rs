//! The events. Every change in x100 is an event appended to a log: one log per project, and one for
//! the daemon. What each tab shows is computed from these.

mod daemon;
mod project;

use serde::{Deserialize, Serialize};

pub use daemon::DaemonEvent;
pub use project::ProjectEvent;

use crate::party::Actor;
use crate::time::Timestamp;

/// An event's place in its log. Each log counts from 1 on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub u64);

/// An event as it is stored and sent: where it sits in its log, when it happened, and who caused
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Envelope<E> {
    pub seq: Seq,
    pub at: Timestamp,
    pub by: Actor,
    pub event: E,
}
