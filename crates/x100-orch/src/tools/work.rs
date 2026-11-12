//! Tasks: `delegate`, `review`, `report`, `raise_task` and `update_task`, and what a merge's
//! outcome does to an accepted task.

use x100_core::agent::{Agent, Position};
use x100_core::effort::Effort;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId, FileId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::party::{Actor, Party};
use x100_core::task::{
    Assignment, Opening, Outcome, Progress, Report, Task, TaskEvent, TaskFailure, TaskState,
    Verdict,
};
use x100_core::time::Clock;
use x100_core::tool::{AgentOutcome, Delegated, Delegatee};

use super::{At, MergePlan, Plan, Refusal, who};
use crate::branches::{Merged, Target};
use crate::ctx::Events;
use crate::gate;
use crate::ids::new_id;
use crate::settle::deliver;

fn task_event(by: Actor, task: TaskId, event: TaskEvent) -> (Actor, ProjectEvent) {
    (by, ProjectEvent::Task { task, event })
}

pub(super) fn delegate(
    at: &At,
    to: &Delegatee,
    task: &Delegated,
    effort: Effort,
) -> Result<Plan, Refusal> {
    let me = at.id();
    let team = &at.project.team;
    let mut events = Vec::new();
    let (assignee, range) = match to {
        Delegatee::Child(a) => {
            at.live_agent(*a)?;
            if *a == me {
                return Err("an agent does not delegate to itself".into());
            }
            if !at.is_my_child(*a) {
                return Err(format!(
                    "{} is not one of your agents: delegate to your own agents or start a new \
                     worker; leads ask each other for help by message",
                    who(at.state, Party::Agent(*a))
                ));
            }
            (*a, team.effort(*a).ok_or("the agent has no effort range")?)
        }
        Delegatee::NewWorker {
            role,
            charter,
            effort: own,
        } => {
            if role.trim().is_empty() {
                return Err("a new worker needs a role".into());
            }
            if own.is_some() && !at.is_ceo() {
                return Err(
                    "only the CEO sets a worker's effort range: leave it out and the worker \
                     has yours"
                        .into(),
                );
            }
            let id = new_id(at.ctx.clock, AgentId::new);
            let agent = Agent {
                id,
                role: role.clone(),
                charter: charter.clone(),
                position: Position::Worker {
                    parent: me,
                    effort: *own,
                },
            };
            events.push((Actor::Agent(me), ProjectEvent::AgentAdded(agent)));
            let range = match own {
                Some(r) => *r,
                None => team.effort(me).ok_or("you have no effort range")?,
            };
            (id, range)
        }
    };
    let assignment = Assignment {
        delegator: Party::Agent(me),
        assignee,
        effort: range.clamp(effort),
    };
    let id = match task {
        Delegated::New {
            title,
            ask,
            attachments,
        } => {
            if title.trim().is_empty() {
                return Err("a task needs a title".into());
            }
            at.files_exist(attachments)?;
            let id = new_id(at.ctx.clock, TaskId::new);
            let task = Task {
                id,
                title: title.clone(),
                ask: ask.clone(),
                part: None,
                raised_by: Party::Agent(me),
                attachments: attachments.clone(),
            };
            events.push((
                Actor::Agent(me),
                ProjectEvent::TaskOpened {
                    task,
                    opening: Opening::Delegated(assignment),
                },
            ));
            id
        }
        Delegated::Raised(id) => {
            match at.state.task(*id).map(|t| &t.state) {
                Some(TaskState::NewIdea) => {}
                Some(other) => {
                    return Err(format!(
                        "task {id} is {}: only a raised task that nobody holds can be delegated",
                        other.name()
                    ));
                }
                None => return Err(format!("there is no task {id}")),
            }
            events.push(task_event(
                Actor::Agent(me),
                *id,
                TaskEvent::Delegated(assignment),
            ));
            *id
        }
    };
    events.push(deliver(
        at.ctx.clock,
        assignee,
        InboxItem::Task { task: id },
    ));
    Ok(Plan {
        commit: vec![me],
        events,
        ..Plan::default()
    })
}

pub(super) fn review(at: &At, task: TaskId, verdict: &Verdict) -> Result<Plan, Refusal> {
    let me = at.id();
    let rec = at
        .state
        .task(task)
        .ok_or_else(|| format!("there is no task {task}"))?;
    let assignment = match &rec.state {
        TaskState::InReview(a, _) => a.clone(),
        other => {
            return Err(format!(
                "task {task} is {}: only a reported task is reviewed",
                other.name()
            ));
        }
    };
    if assignment.delegator != Party::Agent(me) {
        return Err(format!(
            "you did not delegate task {task}: only its delegator reviews it"
        ));
    }
    let mut events = vec![task_event(
        Actor::Agent(me),
        task,
        TaskEvent::Reviewed(verdict.clone()),
    )];
    match verdict {
        Verdict::Accept => Ok(Plan {
            commit: Vec::new(),
            events,
            merge: Some(MergePlan {
                task,
                by: me,
                from: assignment.assignee,
                into: Target::Agent(me),
            }),
            ..Plan::default()
        }),
        Verdict::SendBack { effort, .. } => {
            if let Some(wanted) = effort {
                let range = at
                    .project
                    .team
                    .effort(assignment.assignee)
                    .ok_or("the assignee has no effort range")?;
                let effort = range.clamp(*wanted);
                if effort != assignment.effort {
                    events.push(task_event(
                        Actor::Daemon,
                        task,
                        TaskEvent::EffortChanged(effort),
                    ));
                }
            }
            events.push(deliver(
                at.ctx.clock,
                assignment.assignee,
                InboxItem::Task { task },
            ));
            Ok(Plan {
                commit: Vec::new(),
                events,
                ..Plan::default()
            })
        }
    }
}

pub(super) fn report(
    at: &At,
    task: TaskId,
    outcome: &AgentOutcome,
    summary: &str,
) -> Result<Plan, Refusal> {
    let me = at.id();
    let assignment = at.held_task(task)?;
    let outcome = match outcome {
        AgentOutcome::Completed => Outcome::Completed,
        AgentOutcome::Failed { reason } => Outcome::Failed(TaskFailure::Assignee {
            reason: reason.clone(),
        }),
    };
    let completed = outcome == Outcome::Completed;
    let report = Report {
        outcome,
        summary: summary.to_owned(),
    };
    let mut events = vec![task_event(
        Actor::Agent(me),
        task,
        TaskEvent::Reported(report),
    )];
    let mut merge = None;
    match assignment.delegator {
        Party::Agent(d) => events.push(deliver(at.ctx.clock, d, InboxItem::Report { task })),
        // The builder has no review step: a completed task is accepted as it is reported, and
        // the CEO's branch merged into main. A failed one stays in review, shown in the chat.
        Party::Builder if completed => {
            events.push(task_event(
                Actor::Daemon,
                task,
                TaskEvent::Reviewed(Verdict::Accept),
            ));
            merge = Some(MergePlan {
                task,
                by: me,
                from: me,
                into: Target::Main,
            });
        }
        Party::Builder => {}
    }
    Ok(Plan {
        commit: vec![me],
        events,
        merge,
        ..Plan::default()
    })
}

pub(super) fn raise(
    at: &At,
    title: &str,
    ask: &str,
    part: &Option<String>,
    attachments: &[FileId],
) -> Result<Plan, Refusal> {
    if title.trim().is_empty() {
        return Err("a task needs a title".into());
    }
    at.files_exist(attachments)?;
    let task = Task {
        id: new_id(at.ctx.clock, TaskId::new),
        title: title.to_owned(),
        ask: ask.to_owned(),
        part: part.clone(),
        raised_by: Party::Agent(at.id()),
        attachments: attachments.to_vec(),
    };
    Ok(Plan {
        events: vec![(
            Actor::Agent(at.id()),
            ProjectEvent::TaskOpened {
                task,
                opening: Opening::Raised,
            },
        )],
        ..Plan::default()
    })
}

pub(super) fn update(
    at: &At,
    task: TaskId,
    progress: &Progress,
    effort: Option<Effort>,
) -> Result<Plan, Refusal> {
    let assignment = at.held_task(task)?;
    let mut events = vec![task_event(
        Actor::Agent(at.id()),
        task,
        TaskEvent::ProgressNoted(progress.clone()),
    )];
    if let Some(wanted) = effort {
        let range = at
            .project
            .team
            .effort(at.id())
            .ok_or("you have no effort range")?;
        let effort = range.clamp(wanted);
        if effort != assignment.effort {
            events.push(task_event(
                Actor::Daemon,
                task,
                TaskEvent::EffortChanged(effort),
            ));
        }
    }
    Ok(Plan {
        events,
        ..Plan::default()
    })
}

/// What the merge of an accepted task did: the task is Done, or back with its assignee with the
/// conflict stated. `merge` is the merge's action, whose end is recorded with it.
pub(crate) fn merged(
    state: &ProjectState,
    clock: &dyn Clock,
    plan: &MergePlan,
    merge: ActionId,
    outcome: Result<Merged, String>,
) -> Events {
    let conflict = match outcome {
        Ok(Merged::Into(commit)) => {
            return vec![
                task_event(Actor::Daemon, plan.task, TaskEvent::Merged { commit }),
                gate::finished(merge, None),
            ];
        }
        Ok(Merged::Conflict(conflict)) => conflict,
        Err(error) => error,
    };
    let assignee = state
        .task(plan.task)
        .and_then(|t| t.state.assignment())
        .map_or(plan.from, |a| a.assignee);
    vec![
        task_event(
            Actor::Daemon,
            plan.task,
            TaskEvent::MergeFailed {
                conflict: conflict.clone(),
            },
        ),
        gate::finished(merge, Some(conflict)),
        deliver(clock, assignee, InboxItem::Task { task: plan.task }),
    ]
}

/// The daemon reports a task failed on its assignee's behalf, so it ends in a report to its
/// delegator even when the assignee cannot make one.
pub(crate) fn report_failed(
    state: &ProjectState,
    clock: &dyn Clock,
    task: TaskId,
    failure: TaskFailure,
    summary: String,
) -> Events {
    let Some(TaskState::InProgress(a)) = state.task(task).map(|t| &t.state) else {
        return Vec::new();
    };
    let report = Report {
        outcome: Outcome::Failed(failure),
        summary,
    };
    let mut events = vec![task_event(Actor::Daemon, task, TaskEvent::Reported(report))];
    if let Party::Agent(d) = a.delegator {
        events.push(deliver(clock, d, InboxItem::Report { task }));
    }
    events
}
