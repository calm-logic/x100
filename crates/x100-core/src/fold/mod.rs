//! The folds: the functions that turn a log into the state each tab shows, so the daemon and every
//! client compute it with the same code.
//!
//! There are two states, [`ProjectState`] from a project's log and [`DaemonState`] from the
//! daemon's. Each starts at `Default` and takes events one at a time through `apply`. A state is
//! plain data: it can be sent as a snapshot, and a client that applies the later events to it gets
//! the state the whole log gives.
//!
//! Folding depends only on the events. There is no clock and no randomness here, and every map is
//! ordered, so the same log gives the same state, byte for byte once serialised.
//!
//! The store appends only valid events, so an event the fold cannot apply means the log is corrupt:
//! a machine rejects it, it names a record that does not exist, or it breaks a rule of the team or
//! the budgets. `apply` then returns a [`FoldError`] naming the event and the record, and never
//! skips it. A state that returned an error is part-way through that event and must be discarded.

mod daemon;
mod project;
mod records;
mod views;

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use daemon::{CoolOffRecord, CredentialRecord, DaemonPart, DaemonState, ProjectEntry};
pub use project::{Project, ProjectPart, ProjectState};
pub use records::{
    ActionRecord, AgentRecord, ChatEntry, ChatItem, DriverRecord, FileRecord, JobRecord,
    LoadedSkill, MessageRecord, RunningProcess, SkillRecord, SpecRecord, Spent, TaskRecord,
    TrialRecord,
};
pub use views::{BudgetLine, BudgetView, TeamRow};

use crate::Rejected;
use crate::effort::{Effort, EffortRange};
use crate::event::Seq;
use crate::id::{
    ActionId, AgentId, CredentialRef, DriverId, FileId, HarnessId, JobId, MachineId, MessageId,
    ProjectId, TaskId, TrialId,
};
use crate::inbox::InboxError;
use crate::ledger::LeadsOverTotal;
use crate::spec::SpecKind;
use crate::team::TeamError;

/// One record of either log, as named in changes and errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Record {
    Agent(AgentId),
    Driver(DriverId),
    Task(TaskId),
    Message(MessageId),
    Job(JobId),
    Action(ActionId),
    Spec(SpecKind),
    File(FileId),
    Trial(TrialId),
    Machine(MachineId),
    Harness(HarnessId),
    Credential(CredentialRef),
    Project(ProjectId),
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Record::Agent(id) => write!(f, "agent {id}"),
            Record::Driver(id) => write!(f, "driver {id}"),
            Record::Task(id) => write!(f, "task {id}"),
            Record::Message(id) => write!(f, "message {id}"),
            Record::Job(id) => write!(f, "job {id}"),
            Record::Action(id) => write!(f, "action {id}"),
            Record::Spec(kind) => write!(f, "spec {kind:?}"),
            Record::File(id) => write!(f, "file {id}"),
            Record::Trial(id) => write!(f, "trial {id}"),
            Record::Machine(id) => write!(f, "machine {id}"),
            Record::Harness(id) => write!(f, "harness {id}"),
            Record::Credential(id) => write!(f, "credential {id}"),
            Record::Project(id) => write!(f, "project {id}"),
        }
    }
}

/// What one event changed, so an interface redraws only that and puts a dot on a tab that is not
/// open. `P` names the parts of the interface: [`ProjectPart`] or [`DaemonPart`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes<P> {
    pub parts: BTreeSet<P>,
    pub records: BTreeSet<Record>,
}

impl<P: Ord> Changes<P> {
    fn new() -> Self {
        Self {
            parts: BTreeSet::new(),
            records: BTreeSet::new(),
        }
    }

    fn part(&mut self, part: P) -> &mut Self {
        self.parts.insert(part);
        self
    }

    fn record(&mut self, record: Record) -> &mut Self {
        self.records.insert(record);
        self
    }
}

/// An event the fold could not apply, so the log is corrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldError {
    pub seq: Seq,
    /// The record the event is about, where it names one.
    pub record: Option<Record>,
    pub cause: Corrupt,
}

impl fmt::Display for FoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event {}", self.seq.0)?;
        if let Some(record) = self.record {
            write!(f, " on {record}")?;
        }
        write!(f, ": {}", self.cause)
    }
}

impl std::error::Error for FoldError {}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Corrupt {
    #[error("expected event {} next", .expected.0)]
    OutOfOrder { expected: Seq },
    #[error("the log does not start by creating the project")]
    NotCreated,
    #[error("the project was already created")]
    AlreadyCreated,
    #[error(transparent)]
    Rejected(#[from] Rejected),
    #[error(transparent)]
    Team(#[from] TeamError),
    #[error(transparent)]
    Inbox(#[from] InboxError),
    #[error(transparent)]
    Budgets(#[from] LeadsOverTotal),
    #[error("no such record")]
    Unknown,
    #[error("the record already exists")]
    Duplicate,
    #[error("{0} already acts for this agent")]
    SecondDriver(DriverId),
    #[error("a driver at {effort:?} is outside its agent's range {range:?}")]
    OutOfRange { effort: Effort, range: EffortRange },
    #[error("only a committed trial changes skills and memories")]
    TrialNotCommitted,
}

/// A fold step's failure before the event's place in the log is known.
type Fail = (Option<Record>, Corrupt);

/// Names `record` in a failure.
fn on<E: Into<Corrupt>>(record: Record) -> impl FnOnce(E) -> Fail {
    move |e| (Some(record), e.into())
}

/// The record a lookup by `record` did not find.
fn unknown(record: Record) -> Fail {
    (Some(record), Corrupt::Unknown)
}

/// The sequence number that must come after `last`. Each log counts from 1.
fn expected_after(last: Option<Seq>) -> Seq {
    Seq(last.map_or(1, |s| s.0 + 1))
}
