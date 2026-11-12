use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionEvent};
use crate::agent::{Agent, AgentEvent};
use crate::credential::CredentialUse;
use crate::driver::{Driver, DriverEvent};
use crate::effort::EffortRange;
use crate::file::FileRef;
use crate::git::CommitId;
use crate::harness::OneOffCall;
use crate::id::{ActionId, AgentId, DriverId, FileId, JobId, TaskId, TrialId};
use crate::job::{Job, JobEvent, ProjectJob};
use crate::ledger::{BudgetReached, Charge};
use crate::memory::{NoteName, SkillRef, SkillVersion};
use crate::message::Message;
use crate::party::Party;
use crate::project::{SettingChange, Settings};
use crate::spec::SpecRevision;
use crate::task::{Opening, Task, TaskEvent};
use crate::trial::{Trial, TrialEvent};

/// An event in one project's log.
///
/// A record with a state machine has a creation event carrying what never changes, and a variant
/// naming the record and carrying the machine's event, so replaying the log through the machines
/// checks it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectEvent {
    /// Creating a project creates its CEO.
    Created {
        name: String,
        ceo: AgentId,
        charter: String,
        effort: EffortRange,
        settings: Settings,
    },
    SettingChanged(SettingChange),
    /// The address of the project's development build, from `set_preview`.
    PreviewSet {
        url: String,
    },

    /// A lead named by `form_team` or a team change, or a worker started by `delegate`.
    AgentAdded(Agent),
    Agent {
        agent: AgentId,
        event: AgentEvent,
    },

    /// The router's choice, which names the driver it replaced if this is a handoff.
    DriverStarted(Driver),
    Driver {
        driver: DriverId,
        event: DriverEvent,
    },
    /// The agent's worktree was committed to its hidden checkpoint ref at the end of a turn, or
    /// before a handoff.
    Checkpointed {
        agent: AgentId,
        commit: CommitId,
    },
    /// Before a task goes to an agent that already exists, its delegator's branch is merged into the
    /// agent's, so it starts from current work. The builder's branch is the project's main branch.
    BranchRefreshed {
        agent: AgentId,
        from: Party,
        commit: CommitId,
    },
    OneOffCalled(OneOffCall),
    Charged(Charge),
    BudgetReached(BudgetReached),

    TaskOpened {
        task: Task,
        opening: Opening,
    },
    Task {
        task: TaskId,
        event: TaskEvent,
    },
    MessageSent(Message),
    JobScheduled(Job<ProjectJob>),
    Job {
        job: JobId,
        event: JobEvent,
    },

    ActionProposed(Action),
    Action {
        action: ActionId,
        event: ActionEvent,
    },

    SpecWritten(SpecRevision),
    FileAdded(FileRef),
    /// The builder removed a file to save space.
    FileRemoved {
        file: FileId,
    },

    MemoryWritten {
        agent: AgentId,
        note: NoteName,
        commit: CommitId,
    },
    SkillLoaded {
        agent: AgentId,
        skill: SkillRef,
        version: SkillVersion,
    },
    TrialProposed(Trial),
    Trial {
        trial: TrialId,
        event: TrialEvent,
    },
    /// A skill change reached the main branch. Only a committed trial does that.
    SkillCommitted {
        skill: SkillRef,
        version: SkillVersion,
        commit: CommitId,
        trial: TrialId,
    },
    /// A memory compaction reached the main branch. Only a committed trial does that.
    MemoryCompacted {
        agent: AgentId,
        commit: CommitId,
        trial: TrialId,
    },

    CredentialUsed(CredentialUse),
}
