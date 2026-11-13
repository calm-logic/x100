//! The records a project's state holds: each domain record as it was created, its machine's current
//! state, and what the tabs show about it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionState, Classification};
use crate::agent::AgentState;
use crate::appearance::Appearance;
use crate::driver::{ContextFill, Driver, DriverState};
use crate::event::Seq;
use crate::file::FileRef;
use crate::git::CommitId;
use crate::id::{AgentId, DriverId, InboxItemId, MessageId, TaskId, TrialId, WaitId};
use crate::inbox::{Inbox, InboxItem};
use crate::job::{Job, JobState};
use crate::memory::{NoteName, SkillRef, SkillVersion};
use crate::message::Message;
use crate::money::{Money, Usage};
use crate::party::Actor;
use crate::process::ProcessName;
use crate::spec::SpecRevision;
use crate::task::{Progress, Report, Task, TaskState};
use crate::time::Timestamp;
use crate::trial::{RunScore, Trial, TrialState};
use crate::wait::Wait;

/// Money and tokens spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Spent {
    pub cost: Money,
    pub usage: Usage,
}

impl Spent {
    pub(crate) fn add(&mut self, cost: Money, usage: Usage) {
        self.cost += cost;
        self.usage += usage;
    }
}

/// A member of the team. Its place in the tree, effort range and budget are in the project's
/// [`crate::team::Team`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub role: String,
    pub charter: String,
    pub joined: Timestamp,
    pub appearance: Appearance,
    pub state: AgentState,
    /// Which items it has been given and not yet handled.
    pub inbox: Inbox,
    /// What each unhandled item is.
    pub items: BTreeMap<InboxItemId, InboxItem>,
    /// What it is waiting for.
    pub waits: BTreeMap<WaitId, Wait>,
    /// Tasks assigned to it and in progress, the one it took up last at the end.
    pub open_tasks: Vec<TaskId>,
    /// Every driver it has had, the first one first. Lab drivers included.
    pub drivers: Vec<DriverId>,
    /// Each memory note and the commit that last wrote it.
    pub memories: BTreeMap<NoteName, CommitId>,
    pub skills: Vec<LoadedSkill>,
    /// Processes running in its sandbox, which outlive its drivers.
    pub processes: BTreeMap<ProcessName, RunningProcess>,
    /// What its work drivers cost. Lab runs are on the lab's line, not here.
    pub spent: Spent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningProcess {
    pub command: String,
    pub started: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedSkill {
    pub skill: SkillRef,
    pub version: SkillVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverRecord {
    pub driver: Driver,
    /// Ended or Failed say why it stopped.
    pub state: DriverState,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub turns: u32,
    pub context: Option<ContextFill>,
    pub spent: Spent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task: Task,
    /// The assignment, its effort, and the report are in the state.
    pub state: TaskState,
    pub opened: Timestamp,
    pub progress: Option<Progress>,
    /// The note of the last time it was sent back, while it is back in progress.
    pub sent_back: Option<String>,
    /// The conflict of the last failed merge, while it is back in progress.
    pub conflict: Option<String>,
    pub merged: Option<CommitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRecord {
    pub message: Message,
    pub at: Timestamp,
    /// Its sender waits for a reply to it. For the CEO, a question to the builder.
    pub awaited: bool,
    /// The first message sent in reply to it.
    pub reply: Option<MessageId>,
}

/// One line of the chat between the builder and the CEO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatEntry {
    pub seq: Seq,
    pub at: Timestamp,
    pub item: ChatItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatItem {
    Message(MessageId),
    /// The CEO's report on a task the builder delegated. The builder has no review step, so it
    /// stays as it was reported.
    Report {
        task: TaskId,
        report: Report,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord<D> {
    pub job: Job<D>,
    pub state: JobState,
    pub scheduled: Timestamp,
    pub fired: u32,
    pub last_fired: Option<Timestamp>,
    pub cancelled_by: Option<Actor>,
}

impl<D> JobRecord<D> {
    pub(crate) fn new(job: Job<D>, at: Timestamp) -> Self {
        let state = JobState::initial(&job.when);
        Self {
            job,
            state,
            scheduled: at,
            fired: 0,
            last_fired: None,
            cancelled_by: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub action: Action,
    pub state: ActionState,
    pub proposed: Timestamp,
    /// What it is and why it was flagged, once the gate has classified it.
    pub classification: Option<Classification>,
    pub decided: Option<Timestamp>,
    pub error: Option<String>,
}

/// The latest revision of one spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecRecord {
    pub spec: SpecRevision,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub file: FileRef,
    pub added: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrialRecord {
    pub trial: Trial,
    pub state: TrialState,
    pub proposed: Timestamp,
    /// Every run of both arms, in the order they finished.
    pub runs: Vec<RunScore>,
    pub commit: Option<CommitId>,
}

/// A skill change that reached the main branch, by the trial that put it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRecord {
    pub skill: SkillRef,
    pub version: SkillVersion,
    pub commit: CommitId,
    pub trial: TrialId,
}
