//! What a held action shows the builder, as data for the interface to render.
//!
//! Built from the folded state and the action's record, which the app already has, so the gate
//! stores nothing for it and a client can build the same card the daemon would.

use x100_core::action::{ActionKind, Classification, ClassifiedBy, Request, Risk};
use x100_core::fold::{ActionRecord, ProjectState};
use x100_core::id::{ActionId, AgentId, TaskId};
use x100_core::job::Schedule;
use x100_core::money::Money;
use x100_core::tool::{Delegatee, JobRequest, Proposal, Recipient, ToolCall};

use crate::command::shorten;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub action: ActionId,
    /// What it is, in plain words.
    pub what: String,
    pub kind: ActionKind,
    pub risk: Risk,
    /// Why it was flagged: the rule and what matched it, or what the classifier said.
    pub why: String,
    /// Whether a rule or the classifier decided.
    pub judged_by: ClassifiedBy,
    pub who: Who,
    /// The task the agent was working on (for a merge, the task being merged).
    pub task: Option<TaskLine>,
    /// Set for a change to the team, which is held whatever the level.
    pub always_held: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    pub agent: AgentId,
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    pub id: TaskId,
    pub title: String,
}

impl Shown {
    /// `None` until the action has been classified.
    pub fn of(state: &ProjectState, record: &ActionRecord) -> Option<Shown> {
        let Classification {
            kind,
            risk,
            by,
            why,
        } = record.classification.clone()?;
        let action = &record.action;
        let task = match &action.request {
            Request::Merge { task } => state.task(*task),
            _ => state.current_task(action.agent),
        };
        Some(Shown {
            action: action.id,
            what: what(state, &action.request),
            kind,
            risk,
            why,
            judged_by: by,
            who: Who {
                agent: action.agent,
                role: state.agent(action.agent).map(|a| a.role.clone()),
            },
            task: task.map(|t| TaskLine {
                id: t.task.id,
                title: t.task.title.clone(),
            }),
            always_held: kind == ActionKind::TeamChange,
        })
    }
}

fn agent(state: &ProjectState, id: AgentId) -> String {
    state
        .agent(id)
        .map_or_else(|| id.to_string(), |a| a.role.clone())
}

fn dollars(money: Money) -> String {
    format!("${:.2}", money.micros() as f64 / 1_000_000.0)
}

fn minutes(every: std::time::Duration) -> String {
    let secs = every.as_secs();
    if secs >= 3600 && secs.is_multiple_of(3600) {
        format!("{} hour(s)", secs / 3600)
    } else {
        format!("{} minute(s)", secs.div_ceil(60).max(1))
    }
}

fn title(state: &ProjectState, task: TaskId) -> String {
    state
        .task(task)
        .map_or_else(|| task.to_string(), |t| format!("\"{}\"", t.task.title))
}

fn what(state: &ProjectState, request: &Request) -> String {
    match request {
        Request::Tool { call, .. } => describe(state, call),
        Request::Command { command, .. } => {
            format!("Run the command `{}`", shorten(command, 300))
        }
        Request::Egress { host, method, .. } => {
            format!(
                "Send a {} request to {}",
                method.to_uppercase(),
                shorten(host, 200)
            )
        }
        Request::Merge { task } => {
            let t = title(state, *task);
            match state.task(*task).and_then(|t| t.state.assignment()) {
                Some(a) => match a.delegator {
                    x100_core::party::Party::Builder => {
                        format!("Merge the work on {t} into the main branch")
                    }
                    x100_core::party::Party::Agent(by) => format!(
                        "Merge {}'s work on {t} into {}'s branch",
                        agent(state, a.assignee),
                        agent(state, by)
                    ),
                },
                None => format!("Merge the work on {t}"),
            }
        }
    }
}

/// A tool call in plain words. No wildcard: a new tool needs a sentence here too.
fn describe(state: &ProjectState, call: &ToolCall) -> String {
    use ToolCall as T;
    match call {
        T::Delegate { to, task, .. } => {
            let to = match to {
                Delegatee::Child(a) => agent(state, *a),
                Delegatee::NewWorker { role, .. } => format!("a new worker ({role})"),
            };
            match task {
                x100_core::tool::Delegated::New { title, .. } => {
                    format!("Give the task \"{}\" to {to}", shorten(title, 120))
                }
                x100_core::tool::Delegated::Raised(t) => {
                    format!("Give the task {} to {to}", title(state, *t))
                }
            }
        }
        T::Review { task, verdict } => match verdict {
            x100_core::task::Verdict::Accept => {
                format!("Accept the report on {}", title(state, *task))
            }
            x100_core::task::Verdict::SendBack { .. } => {
                format!("Send {} back for more work", title(state, *task))
            }
        },
        T::Report { task, summary, .. } => {
            format!("Report {}: {}", title(state, *task), shorten(summary, 200))
        }
        T::Message { to, body, .. } => {
            let to = match to {
                Recipient::Builder => "the builder".to_owned(),
                Recipient::Agent(a) => agent(state, *a),
            };
            format!("Send {to} a message: {}", shorten(body, 200))
        }
        T::Wait { task, .. } => format!("Wait, on {}", title(state, *task)),
        T::Schedule { when, does } => {
            let job = match does {
                JobRequest::Wake { agent: a, .. } => format!("wake {}", agent(state, *a)),
                JobRequest::Delegate { to, title, .. } => {
                    format!(
                        "give {} the task \"{}\"",
                        agent(state, *to),
                        shorten(title, 80)
                    )
                }
            };
            match when {
                Schedule::Once { .. } => format!("Schedule a job that will {job} once"),
                Schedule::Every { every, .. } => format!(
                    "Schedule a job that will {job} every {}",
                    minutes(every.get())
                ),
            }
        }
        T::CancelJob { .. } => "Cancel a scheduled job".to_owned(),
        T::RaiseTask { title, .. } => format!("Raise the task \"{}\"", shorten(title, 120)),
        T::UpdateTask { task, .. } => format!("Update the progress of {}", title(state, *task)),
        T::WriteSpec { spec, .. } => format!("Write the {spec:?} document"),
        T::AddFile { name, .. } => format!("Add the file {}", shorten(name, 120)),
        T::SetPreview { url } => format!("Point the preview at {}", shorten(url, 200)),
        T::LoadSkill { skill } => format!("Load the skill {}", skill.name.0),
        T::StartProcess { name, command } => {
            format!(
                "Start `{}` in its sandbox as {}",
                shorten(command, 300),
                name.0
            )
        }
        T::StopProcess { name } => format!("Stop the process {}", name.0),
        T::Remember { note, .. } => format!("Save the memory note {}", note.0),
        T::ReadNote { note } => format!("Read the memory note {}", note.0),
        T::SearchNotes { query } => format!("Search its notes for \"{}\"", shorten(query, 120)),
        T::SearchLog { query, .. } => {
            format!("Search the project's log for \"{}\"", shorten(query, 120))
        }
        T::FormTeam { leads } => {
            let roles: Vec<&str> = leads.iter().map(|l| l.role.as_str()).collect();
            format!(
                "Form a team of {} lead(s): {}",
                leads.len(),
                roles.join(", ")
            )
        }
        T::SetEffort { agent: a, range } => format!(
            "Set {}'s effort range to {:?} through {:?}",
            agent(state, *a),
            range.min(),
            range.max()
        ),
        T::SetBudget { lead, budget } => {
            format!(
                "Set {}'s budget to {}",
                agent(state, *lead),
                dollars(*budget)
            )
        }
        T::ReadLog { .. } => "Read the project's log".to_owned(),
        T::ReadMemories { agent: a } => format!("Read {}'s memories", agent(state, *a)),
        T::ProposeChange(Proposal::Trial { .. }) => "Propose a trial of a skill or memory".into(),
        T::ProposeChange(Proposal::Team {
            changes, telemetry, ..
        }) => format!(
            "Change the team ({} change(s)). Reason: {}",
            changes.len(),
            shorten(telemetry, 300)
        ),
        T::SetUpProject { name, .. } => format!("Set up the project \"{}\"", shorten(name, 120)),
        T::ListIntegrations {} => "List the connected integrations".to_owned(),
        T::LinkProject {
            homebox_project, ..
        } => format!(
            "Link the project to the Homebox project \"{}\"",
            shorten(homebox_project, 120)
        ),
        T::Deploy { environment } => deploy(state, environment.as_deref()),
    }
}

/// What a deploy will do, from where the project stands: published and linked, or not yet.
fn deploy(state: &ProjectState, environment: Option<&str>) -> String {
    let project = state.project();
    let link = project.and_then(|p| p.homebox.as_ref());
    let env = environment
        .or(link.map(|l| l.environment.as_str()))
        .unwrap_or(x100_core::integration::DEFAULT_ENVIRONMENT);
    match (link, project.and_then(|p| p.repository.as_ref())) {
        (Some(l), _) => format!(
            "Push the main branch to {} and deploy the Homebox project \"{}\" ({env}), which is public",
            l.repo_full_name, l.project_name
        ),
        (None, Some(repo)) => format!(
            "Push the main branch to {repo}, add it to Homebox and deploy it ({env}), which is public"
        ),
        (None, None) => format!(
            "Create a private repository on GitHub, push the main branch to it, add it to Homebox \
             and deploy it ({env}), which is public"
        ),
    }
}
