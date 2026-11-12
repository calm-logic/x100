//! The daemon's tools for agents, as pure functions from the folded state and a call to the events
//! that carry it out, or a refusal that says which rule it broke.
//!
//! [`plan`] is the whole decision. It reads nothing but the state and does no I/O, so every rule
//! is tested without a process. What a call needs from git (committing a worktree, merging a
//! branch) is named in the [`Plan`] and done by the desk, which also records the call as an action
//! and appends the events. [`reply`] turns the events into what the driver reads, so the start
//! pack can rebuild a call's result from the log.

mod catalogue;
mod later;
mod schema;
mod setup;
mod talk;
mod team;
mod work;

use serde_json::{Value, json};
use x100_core::agent::{AgentEvent, AgentState};
use x100_core::event::ProjectEvent;
use x100_core::fold::{AgentRecord, Project, ProjectState};
use x100_core::id::{AgentId, FileId, TaskId};
use x100_core::party::Party;
use x100_core::settings::DaemonSettings;
use x100_core::task::{TaskEvent, TaskState};
use x100_core::time::{Clock, Timestamp};
use x100_core::tool::ToolCall;

use crate::branches::Target;
use crate::ctx::Events;

pub(crate) use catalogue::{END_THE_TURN, SET_UP, Tool};
pub(crate) use schema::schema;
pub(crate) use work::{merged, report_failed};

/// Who is calling, and what the call may assume about the world outside the log.
pub(crate) struct Ctx<'a> {
    pub agent: AgentId,
    pub now: Timestamp,
    pub settings: &'a DaemonSettings,
    pub clock: &'a dyn Clock,
    /// For `add_file`: whether the bytes it names are in the project's file store.
    pub file_stored: bool,
}

/// What carrying out a call takes.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Worktrees to commit to their branches first, so their work travels with the call.
    pub commit: Vec<AgentId>,
    pub events: Events,
    /// A merge that follows the events, whose outcome decides what is appended after it.
    pub merge: Option<MergePlan>,
    /// A code directory to make before the events are appended, for a project being set up.
    pub code_dir: Option<NewCodeDir>,
}

/// Where a project being set up gets its code directory: `<root>/<slug>`, or with a number after
/// the slug if that is taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewCodeDir {
    pub root: std::path::PathBuf,
    pub slug: String,
}

/// An accepted task's merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MergePlan {
    pub task: TaskId,
    /// The agent whose acceptance causes it: the delegator, or the CEO for the builder's task.
    pub by: AgentId,
    pub from: AgentId,
    pub into: Target,
}

/// A rule the call broke, in words the agent can act on.
pub(crate) type Refusal = String;

/// Decides a call. The caller's driver is assumed to be the agent's current one; the desk checks.
pub(crate) fn plan(state: &ProjectState, ctx: &Ctx, call: &ToolCall) -> Result<Plan, Refusal> {
    let tool = Tool::of(call);
    if let Some(milestone) = later::arrives_with(tool) {
        return Err(format!(
            "`{}` is not available yet: it arrives with {milestone}",
            tool.name()
        ));
    }
    let project = state.project().ok_or("the project is not created yet")?;
    let me = state
        .agent(ctx.agent)
        .ok_or("the calling agent is not on the team")?;
    if me.state == AgentState::Retired {
        return Err("a retired agent has no tools".into());
    }
    let at = At {
        state,
        project,
        me,
        ctx,
    };
    if tool.ceo_only() && !at.is_ceo() {
        return Err(format!("only the CEO calls `{}`", tool.name()));
    }
    match call {
        ToolCall::Delegate { to, task, effort } => work::delegate(&at, to, task, *effort),
        ToolCall::Review { task, verdict } => work::review(&at, *task, verdict),
        ToolCall::Report {
            task,
            outcome,
            summary,
        } => work::report(&at, *task, outcome, summary),
        ToolCall::RaiseTask {
            title,
            ask,
            part,
            attachments,
        } => work::raise(&at, title, ask, part, attachments),
        ToolCall::UpdateTask {
            task,
            progress,
            effort,
        } => work::update(&at, *task, progress, *effort),
        ToolCall::Message {
            to,
            body,
            attachments,
            in_reply_to,
        } => talk::message(&at, *to, body, attachments, *in_reply_to),
        ToolCall::Wait {
            on,
            deadline,
            task,
            progress,
        } => talk::wait(&at, *on, *deadline, *task, progress),
        ToolCall::Schedule { when, does } => talk::schedule(&at, *when, does),
        ToolCall::CancelJob { job } => talk::cancel(&at, *job),
        ToolCall::FormTeam { leads } => team::form(&at, leads),
        ToolCall::SetEffort { agent, range } => team::set_effort(&at, *agent, *range),
        ToolCall::SetBudget { lead, budget } => team::set_budget(&at, *lead, *budget),
        ToolCall::WriteSpec { spec, status, body } => team::write_spec(&at, *spec, *status, body),
        ToolCall::AddFile {
            name,
            media_type,
            size,
            hash,
        } => team::add_file(&at, name, media_type, *size, hash),
        ToolCall::SetPreview { url } => team::set_preview(&at, url),
        ToolCall::SetUpProject {
            name,
            intent,
            brief,
        } => setup::set_up(&at, name, intent, brief.as_deref()),
        ToolCall::StartProcess { .. }
        | ToolCall::StopProcess { .. }
        | ToolCall::LoadSkill { .. }
        | ToolCall::Remember { .. }
        | ToolCall::ReadLog { .. }
        | ToolCall::ReadMemories { .. }
        | ToolCall::ProposeChange(_) => unreachable!("answered by later::arrives_with"),
    }
}

/// Everything a handler reads.
pub(crate) struct At<'a> {
    pub state: &'a ProjectState,
    pub project: &'a Project,
    pub me: &'a AgentRecord,
    pub ctx: &'a Ctx<'a>,
}

impl At<'_> {
    pub fn id(&self) -> AgentId {
        self.me.id
    }

    pub fn is_ceo(&self) -> bool {
        self.project.team.ceo() == self.me.id
    }

    /// The agent, if it is on the team and not retired.
    pub fn live_agent(&self, agent: AgentId) -> Result<&AgentRecord, Refusal> {
        match self.state.agent(agent) {
            Some(a) if a.state != AgentState::Retired => Ok(a),
            Some(_) => Err(format!("{agent} is retired")),
            None => Err(format!("{agent} is not on the team")),
        }
    }

    pub fn is_my_child(&self, agent: AgentId) -> bool {
        self.project.team.parent(agent) == Some(self.id())
    }

    pub fn files_exist(&self, files: &[FileId]) -> Result<(), Refusal> {
        match files.iter().find(|f| self.state.file(**f).is_none()) {
            Some(f) => Err(format!("there is no file {f}")),
            None => Ok(()),
        }
    }

    /// A task the caller holds: assigned to it and in progress.
    pub fn held_task(&self, task: TaskId) -> Result<x100_core::task::Assignment, Refusal> {
        let rec = self
            .state
            .task(task)
            .ok_or_else(|| format!("there is no task {task}"))?;
        match &rec.state {
            TaskState::InProgress(a) if a.assignee == self.id() => Ok(a.clone()),
            TaskState::InProgress(_) => Err(format!(
                "task {task} is not yours: only the agent it was delegated to can do that"
            )),
            other => Err(format!("task {task} is {}, not in progress", other.name())),
        }
    }
}

/// What a carried-out call tells the driver: the records it made, as JSON so a script can pick a
/// field out. `events` are the call's own events, and the merge's when it had one.
pub(crate) fn reply(call: &ToolCall, events: &[ProjectEvent]) -> String {
    let task = || {
        events.iter().find_map(|e| match e {
            ProjectEvent::TaskOpened { task, .. } => Some(task.id),
            ProjectEvent::Task { task, .. } => Some(*task),
            _ => None,
        })
    };
    let merge = || {
        events.iter().find_map(|e| match e {
            ProjectEvent::Task {
                event: TaskEvent::Merged { commit },
                ..
            } => Some(json!({ "merged": commit.0 })),
            ProjectEvent::Task {
                event: TaskEvent::MergeFailed { conflict },
                ..
            } => Some(json!({ "merge_failed": conflict })),
            _ => None,
        })
    };
    let mut out = match call {
        ToolCall::Delegate { .. } => {
            let agent = events.iter().find_map(|e| match e {
                ProjectEvent::Agent {
                    agent,
                    event: AgentEvent::Delivered { .. },
                } => Some(*agent),
                _ => None,
            });
            json!({ "task": task(), "agent": agent })
        }
        ToolCall::Review { task, .. } | ToolCall::Report { task, .. } => json!({ "task": task }),
        ToolCall::RaiseTask { .. } => json!({ "task": task() }),
        ToolCall::UpdateTask { task, .. } => json!({ "task": task }),
        ToolCall::Message { .. } => json!({
            "message": events.iter().find_map(|e| match e {
                ProjectEvent::MessageSent(m) => Some(m.id),
                _ => None,
            })
        }),
        ToolCall::Wait { .. } => match events.iter().find_map(|e| match e {
            ProjectEvent::Agent {
                event: AgentEvent::WaitOpened(w),
                ..
            } => Some(w),
            _ => None,
        }) {
            Some(w) => json!({ "wait": w.id, "deadline": w.deadline.as_millis() }),
            None => json!({}),
        },
        ToolCall::Schedule { .. } => json!({
            "job": events.iter().find_map(|e| match e {
                ProjectEvent::JobScheduled(j) => Some(j.id),
                _ => None,
            })
        }),
        ToolCall::CancelJob { job } => json!({ "job": job, "cancelled": true }),
        ToolCall::FormTeam { .. } => {
            let leads: serde_json::Map<_, _> = events
                .iter()
                .filter_map(|e| match e {
                    ProjectEvent::AgentAdded(a) => Some((a.role.clone(), json!(a.id))),
                    _ => None,
                })
                .collect();
            Value::Object(leads)
        }
        ToolCall::SetEffort { agent, .. } => json!({ "agent": agent }),
        ToolCall::SetBudget { lead, .. } => json!({ "agent": lead }),
        ToolCall::WriteSpec { .. } => json!({
            "revision": events.iter().find_map(|e| match e {
                ProjectEvent::SpecWritten(s) => Some(s.revision),
                _ => None,
            })
        }),
        ToolCall::AddFile { .. } => json!({
            "file": events.iter().find_map(|e| match e {
                ProjectEvent::FileAdded(f) => Some(f.id),
                _ => None,
            })
        }),
        ToolCall::SetPreview { url } => json!({ "preview": url }),
        ToolCall::SetUpProject { .. } => {
            let mut out = json!({ "name": events.iter().find_map(|e| match e {
                ProjectEvent::SetUp(i) | ProjectEvent::Renamed(i) => Some(&i.name),
                _ => None,
            }) });
            if let Some(task) = task() {
                out["task"] = json!(task);
            }
            out
        }
        _ => json!({}),
    };
    if let (Some(extra), Value::Object(m)) = (merge(), &mut out)
        && let Value::Object(e) = extra
    {
        m.extend(e);
    }
    out.to_string()
}

/// Who a party is, for text an agent reads.
pub(crate) fn who(state: &ProjectState, party: Party) -> String {
    match party {
        Party::Builder => "the builder".into(),
        Party::Agent(a) => state
            .agent(a)
            .map_or_else(|| a.to_string(), |r| format!("{} ({a})", r.role)),
    }
}

#[cfg(test)]
mod tests;
