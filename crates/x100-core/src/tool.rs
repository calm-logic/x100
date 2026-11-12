//! The tools the daemon serves to every driver over MCP, as calls with their arguments. Every call
//! is an [`crate::action::Action`], so the gate sees it and the log records it.

use serde::{Deserialize, Serialize};

use crate::effort::{Effort, EffortRange};
use crate::event::Seq;
use crate::file::ContentHash;
use crate::id::{AgentId, FileId, JobId, MessageId, TaskId};
use crate::job::Schedule;
use crate::memory::{NoteName, SkillRef};
use crate::money::Money;
use crate::process::ProcessName;
use crate::spec::{SpecKind, SpecStatus};
use crate::task::{Progress, Verdict};
use crate::team::{LeadSpec, TeamChange};
use crate::time::Timestamp;
use crate::trial::{EvalSet, TrialChange};
use crate::wait::WaitOn;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCall {
    /// Give a task to one of the agent's children, or start a new worker to take it.
    Delegate {
        to: Delegatee,
        task: Delegated,
        effort: Effort,
    },
    /// Accept a report on a task the agent delegated, or send the task back.
    Review {
        task: TaskId,
        verdict: Verdict,
    },
    /// End a task the agent was given.
    Report {
        task: TaskId,
        outcome: AgentOutcome,
        summary: String,
    },
    Message {
        to: Recipient,
        body: String,
        attachments: Vec<FileId>,
        in_reply_to: Option<MessageId>,
    },
    /// `wait` cannot be called without a progress note. A missing deadline gets the default.
    Wait {
        on: WaitOn,
        deadline: Option<Timestamp>,
        task: TaskId,
        progress: Progress,
    },
    /// For the agent itself or one of its children.
    Schedule {
        when: Schedule,
        does: JobRequest,
    },
    CancelJob {
        job: JobId,
    },
    RaiseTask {
        title: String,
        ask: String,
        part: Option<String>,
        attachments: Vec<FileId>,
    },
    /// `effort` is set when the assignee says the task needs a different effort. The daemon clamps
    /// it into the agent's range and records it as [`crate::task::TaskEvent::EffortChanged`].
    UpdateTask {
        task: TaskId,
        progress: Progress,
        effort: Option<Effort>,
    },
    WriteSpec {
        spec: SpecKind,
        status: SpecStatus,
        body: String,
    },
    /// The bytes are already in the file store under `hash`.
    AddFile {
        name: String,
        media_type: String,
        size: u64,
        hash: ContentHash,
    },
    SetPreview {
        url: String,
    },
    LoadSkill {
        skill: SkillRef,
    },
    /// Run a command in the agent's sandbox, owned by the sandbox so it outlives the driver.
    StartProcess {
        name: ProcessName,
        command: String,
    },
    StopProcess {
        name: ProcessName,
    },
    Remember {
        note: NoteName,
        body: String,
    },
    /// The CEO only. The first team needs no approval.
    FormTeam {
        leads: Vec<LeadSpec>,
    },
    /// The CEO only.
    SetEffort {
        agent: AgentId,
        range: EffortRange,
    },
    /// The CEO only.
    SetBudget {
        lead: AgentId,
        budget: Money,
    },
    /// The CEO only, for reflection: the project's log between two points.
    ReadLog {
        from: Seq,
        to: Option<Seq>,
    },
    /// The CEO only, for reflection.
    ReadMemories {
        agent: AgentId,
    },
    /// The CEO only, for reflection.
    ProposeChange(Proposal),
    /// The CEO only. The first call sets the project up: it records the name and the intent,
    /// makes the code directory, and opens the builder's task with `brief` as what is asked. A
    /// later call changes the name or the intent and nothing else, so `brief` is for the first.
    SetUpProject {
        name: String,
        intent: String,
        brief: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delegatee {
    Child(AgentId),
    /// Delegating to a new worker starts it. There is no separate tool to start a worker.
    NewWorker {
        role: String,
        charter: String,
        effort: Option<EffortRange>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delegated {
    New {
        title: String,
        ask: String,
        attachments: Vec<FileId>,
    },
    /// A task raised earlier as a new idea.
    Raised(TaskId),
}

/// How an agent ends its own task. Failures the daemon reports for it are not the agent's to claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    Completed,
    Failed { reason: String },
}

/// Only the CEO may message the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipient {
    Agent(AgentId),
    Builder,
}

/// What an agent may schedule. The daemon's routines are not among them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobRequest {
    Wake {
        agent: AgentId,
        note: String,
    },
    Delegate {
        to: AgentId,
        title: String,
        ask: String,
        effort: Effort,
    },
}

/// What reflection proposes. Skill and memory changes go on trial; a team change goes to the
/// builder as a held action, with the telemetry that prompted it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proposal {
    Trial {
        change: TrialChange,
        eval: EvalSet,
    },
    Team {
        changes: Vec<TeamChange>,
        telemetry: String,
        team_toml: String,
    },
}
