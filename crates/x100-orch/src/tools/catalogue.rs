//! The tools as an agent's model sees them: one per [`ToolCall`] variant, with the name it is called
//! by, who is offered it, and a description written for a model. Their argument schemas are in
//! [`super::schema`]. Both transports read this: MCP lists and parses from it, and the fake
//! harness's calls are already [`ToolCall`]s.

use serde_json::{Value, json};
use x100_core::tool::ToolCall;

use super::later::arrives_with;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Tool {
    Delegate,
    Review,
    Report,
    Message,
    Wait,
    Schedule,
    CancelJob,
    RaiseTask,
    UpdateTask,
    WriteSpec,
    AddFile,
    SetPreview,
    LoadSkill,
    StartProcess,
    StopProcess,
    Remember,
    FormTeam,
    SetEffort,
    SetBudget,
    ReadLog,
    ReadMemories,
    ProposeChange,
    SetUpProject,
}

/// What the CEO is told once its call has set the project up.
pub(crate) const SET_UP: &str = "The project is set up and the builder's task is open. Your work \
     on it continues in the project's own working directory, starting with your next turn, where \
     the task reaches you. End this turn now, with at most a short line to the builder and no \
     further tool calls.";

/// What the model is told after a call that ends its part in the turn.
pub(crate) const END_THE_TURN: &str = "Your part in this turn is done: end the turn now, with at \
     most a short closing line and no further tool calls. You get a new turn when something \
     reaches your inbox.";

impl Tool {
    pub const ALL: [Tool; 23] = [
        Tool::Delegate,
        Tool::Review,
        Tool::Report,
        Tool::Message,
        Tool::Wait,
        Tool::Schedule,
        Tool::CancelJob,
        Tool::RaiseTask,
        Tool::UpdateTask,
        Tool::WriteSpec,
        Tool::AddFile,
        Tool::SetPreview,
        Tool::LoadSkill,
        Tool::StartProcess,
        Tool::StopProcess,
        Tool::Remember,
        Tool::FormTeam,
        Tool::SetEffort,
        Tool::SetBudget,
        Tool::ReadLog,
        Tool::ReadMemories,
        Tool::ProposeChange,
        Tool::SetUpProject,
    ];

    pub fn of(call: &ToolCall) -> Tool {
        match call {
            ToolCall::Delegate { .. } => Tool::Delegate,
            ToolCall::Review { .. } => Tool::Review,
            ToolCall::Report { .. } => Tool::Report,
            ToolCall::Message { .. } => Tool::Message,
            ToolCall::Wait { .. } => Tool::Wait,
            ToolCall::Schedule { .. } => Tool::Schedule,
            ToolCall::CancelJob { .. } => Tool::CancelJob,
            ToolCall::RaiseTask { .. } => Tool::RaiseTask,
            ToolCall::UpdateTask { .. } => Tool::UpdateTask,
            ToolCall::WriteSpec { .. } => Tool::WriteSpec,
            ToolCall::AddFile { .. } => Tool::AddFile,
            ToolCall::SetPreview { .. } => Tool::SetPreview,
            ToolCall::LoadSkill { .. } => Tool::LoadSkill,
            ToolCall::StartProcess { .. } => Tool::StartProcess,
            ToolCall::StopProcess { .. } => Tool::StopProcess,
            ToolCall::Remember { .. } => Tool::Remember,
            ToolCall::FormTeam { .. } => Tool::FormTeam,
            ToolCall::SetEffort { .. } => Tool::SetEffort,
            ToolCall::SetBudget { .. } => Tool::SetBudget,
            ToolCall::ReadLog { .. } => Tool::ReadLog,
            ToolCall::ReadMemories { .. } => Tool::ReadMemories,
            ToolCall::ProposeChange(_) => Tool::ProposeChange,
            ToolCall::SetUpProject { .. } => Tool::SetUpProject,
        }
    }

    pub fn named(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The name agents call it by, which is also the variant's name in [`ToolCall`]'s serde form.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Delegate => "delegate",
            Tool::Review => "review",
            Tool::Report => "report",
            Tool::Message => "message",
            Tool::Wait => "wait",
            Tool::Schedule => "schedule",
            Tool::CancelJob => "cancel_job",
            Tool::RaiseTask => "raise_task",
            Tool::UpdateTask => "update_task",
            Tool::WriteSpec => "write_spec",
            Tool::AddFile => "add_file",
            Tool::SetPreview => "set_preview",
            Tool::LoadSkill => "load_skill",
            Tool::StartProcess => "start_process",
            Tool::StopProcess => "stop_process",
            Tool::Remember => "remember",
            Tool::FormTeam => "form_team",
            Tool::SetEffort => "set_effort",
            Tool::SetBudget => "set_budget",
            Tool::ReadLog => "read_log",
            Tool::ReadMemories => "read_memories",
            Tool::ProposeChange => "propose_change",
            Tool::SetUpProject => "set_up_project",
        }
    }

    /// Offered to the CEO's drivers alone, and refused to anyone else.
    pub fn ceo_only(self) -> bool {
        matches!(
            self,
            Tool::FormTeam
                | Tool::SetEffort
                | Tool::SetBudget
                | Tool::ReadLog
                | Tool::ReadMemories
                | Tool::ProposeChange
                | Tool::SetUpProject
        )
    }

    /// Whether calling it does something yet. A tool a later milestone builds is not offered.
    pub fn served(self) -> bool {
        arrives_with(self).is_none()
    }

    /// After it, the agent has nothing left to do in the turn.
    pub fn ends_turn(self) -> bool {
        matches!(self, Tool::Wait | Tool::Report)
    }

    /// The call these arguments make, or why they do not fit the tool.
    pub fn parse(self, arguments: Value) -> Result<ToolCall, String> {
        let call: ToolCall = serde_json::from_value(json!({ self.name(): arguments }))
            .map_err(|e| format!("the arguments do not fit `{}`: {e}", self.name()))?;
        debug_assert_eq!(Tool::of(&call), self);
        Ok(call)
    }

    pub fn description(self) -> &'static str {
        match self {
            Tool::Delegate => {
                "Give a task to one of your own agents, or start a new worker to take it. Use it \
                 for work that should go on beside yours or needs another role. Leads ask each \
                 other for help with `message`, not with this. The task's effort is clamped into \
                 the assignee's range. Returns the task's id and the assignee's id. The report \
                 comes to your inbox; `wait` on it when you have nothing else to do."
            }
            Tool::Review => {
                "Accept the report on a task you delegated, or send the task back with a note \
                 saying what is missing. Accepting merges the assignee's work into your branch. \
                 Returns the task's id, and the merge's commit or the conflict that stopped it, in \
                 which case the task goes back to the assignee."
            }
            Tool::Report => {
                "End a task you were given, as completed or failed, with a summary for whoever \
                 delegated it. Your working directory is committed with the report, so there is \
                 no need to commit first. For the builder's task, the summary goes to the builder \
                 and your work is merged into the project's main branch. This ends your part in \
                 the turn. Returns the task's id."
            }
            Tool::Message => {
                "Send a message to another agent, or, for the CEO alone, to the builder. Use it to \
                 ask or to tell; to hand work over use `delegate`. Returns the message's id. To \
                 stop until the answer comes, `wait` on a reply to it."
            }
            Tool::Wait => {
                "Stop working until something happens: the report on a task you delegated, a \
                 reply to a message you sent, a job firing, or the decision on a held action. \
                 Give your progress note on the task you hold, so whoever picks it up next knows \
                 where it stands. Anything new in your inbox wakes you too, and so does the \
                 deadline, which defaults to the daemon's. This ends your part in the turn. \
                 Returns the wait's id and its deadline in milliseconds since the Unix epoch."
            }
            Tool::Schedule => {
                "Schedule a job for yourself or one of your agents: wake an agent with a note, or \
                 delegate it a fresh task at each firing. Once at a time, or repeating at a fixed \
                 interval from a first time. Times are milliseconds since the Unix epoch. Returns \
                 the job's id; `wait` on it to stop until it fires."
            }
            Tool::CancelJob => {
                "Cancel a job you scheduled that has not fired for the last time. A wait on it \
                 ends. Returns the job's id."
            }
            Tool::RaiseTask => {
                "Write down a task nobody holds yet, as an idea to delegate later. Returns the new \
                 task's id."
            }
            Tool::UpdateTask => {
                "Bring the progress note of a task you hold up to date: what is done, what is \
                 next, what was decided, and the share done if you can say. Do it after each real \
                 step: a driver that takes over after a failure starts from this note. Set \
                 `effort` if the task needs a different effort. Returns the task's id."
            }
            Tool::WriteSpec => {
                "Write a new revision of one of the project's specs, the whole document in \
                 Markdown: the BRD (business requirements, the CEO's), the PRD (product \
                 requirements) or the ADD (architecture design, a lead's or the CEO's). Returns \
                 the revision's number."
            }
            Tool::AddFile => {
                "Record a file whose bytes are already in the project's file store, by name and \
                 content hash, so tasks and messages can attach it. Returns the file's id."
            }
            Tool::SetPreview => {
                "Point the project's preview at an address, such as a dev server you started, so \
                 the builder can open it. Returns the address."
            }
            Tool::LoadSkill => "Load a skill, shared or your own, into your instructions.",
            Tool::StartProcess => {
                "Start a long-running command, such as a dev server, in your sandbox, run by \
                 `sh -c` in your working directory. It is owned by the sandbox, so it keeps \
                 running after your turn and for your next driver; commands you start yourself \
                 end with you. Returns the file its output goes to."
            }
            Tool::StopProcess => {
                "Stop a process you started with `start_process`, and everything it started."
            }
            Tool::Remember => {
                "Write a note to your memory, which every later driver of yours reads."
            }
            Tool::FormTeam => {
                "CEO only. Form the first team: each lead with a role, a charter, an effort range \
                 and a budget in millionths of a dollar. The leads' budgets together must fit the \
                 project's. Allowed once: later changes go to the builder. Returns each lead's id \
                 by role."
            }
            Tool::SetEffort => {
                "CEO only. Set the range of effort an agent's drivers may run at. Returns the \
                 agent's id."
            }
            Tool::SetBudget => {
                "CEO only. Set a lead's budget in millionths of a dollar. The leads' budgets \
                 together must fit the project's. Returns the lead's id."
            }
            Tool::ReadLog => "CEO only, for reflection. Read the project's log between two points.",
            Tool::ReadMemories => "CEO only, for reflection. Read an agent's memories.",
            Tool::ProposeChange => {
                "CEO only, for reflection. Propose a change to a skill or a memory, put on trial, \
                 or to the team, which goes to the builder."
            }
            Tool::SetUpProject => {
                "CEO only. Name a new project and say what it is for, once the builder has told \
                 you enough to know. The first call sets the project up: it records the name and \
                 the intent, makes the project's code directory, and opens the builder's task \
                 with `brief` as what is asked, carrying the files the builder attached. Your \
                 work then continues on your next turn, in the project's working directory. \
                 Until this is called there is no task: do not plan, write specs or form a team \
                 before it. Call it again later only to change the name or the intent; that \
                 changes nothing else. Returns the task's id the first time."
            }
        }
    }
}
