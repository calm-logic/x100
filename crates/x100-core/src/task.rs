//! Tasks: raised and delegated work, and the report that ends each one.

use serde::{Deserialize, Serialize};

use crate::driver::{Failure, Incompatible};
use crate::effort::Effort;
use crate::git::CommitId;
use crate::id::{AgentId, FileId, TaskId};
use crate::machine::{MachineKind, Rejected};
use crate::money::Percent;
use crate::party::Party;

/// A task as it is opened: what is asked and by whom. Never changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub ask: String,
    /// The part of the product it belongs to, where the raiser said.
    pub part: Option<String>,
    pub raised_by: Party,
    pub attachments: Vec<FileId>,
}

/// How a task is opened: raised as a new idea, or delegated straight away, which starts it In
/// progress.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opening {
    Raised,
    Delegated(Assignment),
}

/// Who gave the task to whom, at what effort. The assignee owns the task until it reports, and the
/// report goes back to the delegator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Assignment {
    pub delegator: Party,
    pub assignee: AgentId,
    pub effort: Effort,
}

/// The driver's own short account of where the task stands. Kept current with `update_task`, and
/// required by `wait`, so a new driver can pick the task up.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Progress {
    pub note: String,
    pub done: Option<Percent>,
}

/// How a task ends: a report to its delegator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Report {
    pub outcome: Outcome,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Failed(TaskFailure),
}

/// Why a task failed. Only the first comes from the assignee. The daemon reports the rest on its
/// behalf, so a task ends in a report even when the assignee cannot make one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskFailure {
    /// The assignee reported that it could not do the task.
    Assignee { reason: String },
    /// Its drivers failed this many times in a row, with no turn completed between.
    DriverFailures(Vec<Failure>),
    /// No compatible driver was found before the wait for one reached its deadline.
    NoDriver(Vec<Incompatible>),
    /// It took this many turns in a row without reporting or waiting.
    NoProgress { turns: u32 },
}

/// What the delegator decides on a report, through `review`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accept,
    /// `effort` is set when the delegator asks for more. The daemon clamps it into the assignee's
    /// range and records it as [`TaskEvent::EffortChanged`].
    SendBack {
        note: String,
        effort: Option<Effort>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    NewIdea,
    InProgress(Assignment),
    InReview(Assignment, Report),
    /// The assignee's branch is being merged into the delegator's, or the CEO's into main.
    Promoting(Assignment, Report),
    Done(Assignment, Report),
}

impl TaskState {
    pub fn initial(opening: &Opening) -> TaskState {
        match opening {
            Opening::Raised => TaskState::NewIdea,
            Opening::Delegated(a) => TaskState::InProgress(a.clone()),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            TaskState::NewIdea => "NewIdea",
            TaskState::InProgress(_) => "InProgress",
            TaskState::InReview(..) => "InReview",
            TaskState::Promoting(..) => "Promoting",
            TaskState::Done(..) => "Done",
        }
    }

    pub fn assignment(&self) -> Option<&Assignment> {
        match self {
            TaskState::NewIdea => None,
            TaskState::InProgress(a)
            | TaskState::InReview(a, _)
            | TaskState::Promoting(a, _)
            | TaskState::Done(a, _) => Some(a),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEvent {
    Delegated(Assignment),
    ProgressNoted(Progress),
    EffortChanged(Effort),
    Reported(Report),
    Reviewed(Verdict),
    Merged {
        commit: CommitId,
    },
    /// The merge conflicted. The task goes back to the assignee with the conflict stated.
    MergeFailed {
        conflict: String,
    },
}

impl TaskEvent {
    pub fn name(&self) -> &'static str {
        match self {
            TaskEvent::Delegated(_) => "Delegated",
            TaskEvent::ProgressNoted(_) => "ProgressNoted",
            TaskEvent::EffortChanged(_) => "EffortChanged",
            TaskEvent::Reported(_) => "Reported",
            TaskEvent::Reviewed(Verdict::Accept) => "Reviewed(Accept)",
            TaskEvent::Reviewed(Verdict::SendBack { .. }) => "Reviewed(SendBack)",
            TaskEvent::Merged { .. } => "Merged",
            TaskEvent::MergeFailed { .. } => "MergeFailed",
        }
    }
}

/// The task machine. New idea → In progress → In review → Promoting → Done. A report moves it to In
/// review, a failed one included. Accepting moves it to Promoting while the branch is merged, then
/// to Done. In review → In progress when sent back. Promoting → In progress when the merge fails.
pub fn next(state: &TaskState, event: &TaskEvent) -> Result<TaskState, Rejected> {
    use TaskEvent as E;
    use TaskState as S;
    let next = match (state, event) {
        (S::NewIdea, E::Delegated(a)) => Some(S::InProgress(a.clone())),
        (S::InProgress(a), E::ProgressNoted(_)) => Some(S::InProgress(a.clone())),
        (S::InProgress(a), E::EffortChanged(effort)) => Some(S::InProgress(Assignment {
            effort: *effort,
            ..a.clone()
        })),
        (S::InProgress(a), E::Reported(r)) => Some(S::InReview(a.clone(), r.clone())),
        (S::InReview(a, r), E::Reviewed(Verdict::Accept)) => {
            Some(S::Promoting(a.clone(), r.clone()))
        }
        (S::InReview(a, _), E::Reviewed(Verdict::SendBack { .. })) => {
            Some(S::InProgress(a.clone()))
        }
        (S::Promoting(a, r), E::Merged { .. }) => Some(S::Done(a.clone(), r.clone())),
        (S::Promoting(a, _), E::MergeFailed { .. }) => Some(S::InProgress(a.clone())),
        _ => None,
    };
    next.ok_or(Rejected {
        machine: MachineKind::Task,
        state: state.name(),
        event: event.name(),
    })
}
