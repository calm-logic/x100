//! The builder's Approve or Deny of a held action, and what each does, by the kind of request
//! (`x100_gate::resume` says which): a held tool call is carried out by the daemon as the agent;
//! a command or a request out is allowed once, for the agent's next try; a merge is done, or on
//! Deny the task goes back to its assignee. Every decision reaches the agent through its inbox,
//! with what came of it, so it reaches whichever driver the agent has by then.

use x100_core::action::{ActionEvent, ActionState, Decision, Request};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::party::Actor;
use x100_core::task::TaskEvent;
use x100_core::time::Clock;
use x100_gate::{Effect, tell_agent};

use crate::Shared;
use crate::branches;
use crate::builder::Refused;
use crate::ctx::{Events, ProjectCtx, Writer, git_of};
use crate::gate;
use crate::settle::deliver;
use crate::tools::{self, MergePlan};

const COMMAND_ONCE: &str = "The builder approved it. Run exactly the same command once more and \
     it is allowed, once.";
const EGRESS_ONCE: &str = "The builder approved it. Send exactly the same request once more and \
     it goes through, once.";
const NOT_CARRIED_OUT: &str = "It was not carried out: the daemon stopped before it could be. Ask \
     again if it is still needed.";

/// Records the builder's decision on held action `id` and does what it calls for. Blocks.
pub(crate) fn decide(
    shared: &Shared,
    ctx: &std::sync::Arc<ProjectCtx>,
    id: ActionId,
    decision: Decision,
) -> Result<(), Refused> {
    let w = ctx.writer();
    let record = w
        .read(|s| s.action(id).cloned())
        .ok_or_else(|| Refused::NotFound(format!("no action {id}")))?;
    if record.state != ActionState::Held {
        return Err(Refused::WrongState(format!(
            "action {id} is {}: only a held action is decided",
            record.state.name()
        )));
    }
    let action = record.action;
    let clock = w.clock();
    let decided = (
        Actor::Builder,
        ProjectEvent::Action {
            action: id,
            event: match decision {
                Decision::Approved => ActionEvent::Approved,
                Decision::Denied => ActionEvent::Denied,
            },
        },
    );
    let tell = |result: Option<&str>| {
        deliver(
            clock,
            action.agent,
            tell_agent(id, decision, result.map(str::to_owned)),
        )
    };
    let events = match x100_gate::resume(&action.request).effect(decision) {
        Effect::RunToolCall => {
            // The desk appends the approval with what came of the call, and tells the agent.
            if let Err(why) = shared.desk.approved(ctx, &w, &action, vec![decided]) {
                eprintln!("x100: the approved action {id} was not carried out: {why}");
            }
            return still_held(&w, id);
        }
        Effect::AllowCommandOnce => vec![decided, tell(Some(COMMAND_ONCE))],
        Effect::AllowEgressOnce => vec![decided, tell(Some(EGRESS_ONCE))],
        Effect::Drop => vec![decided, tell(None)],
        Effect::MergeBranch => {
            let Request::Merge { task } = action.request else {
                unreachable!("only a merge is merged")
            };
            return merge(&w, id, task, decided, &tell);
        }
        Effect::SendTaskBack => {
            let Request::Merge { task } = action.request else {
                unreachable!("only a merge sends a task back")
            };
            let mut events = vec![decided];
            events.extend(w.read(|s| sent_back(s, clock, task, "The builder denied the merge.")));
            events.push(tell(Some(
                "The merge was not done: the task went back to its assignee.",
            )));
            events
        }
    };
    w.append(events)?;
    Ok(())
}

/// The decision could not be recorded if the action is still held.
fn still_held(w: &Writer, id: ActionId) -> Result<(), Refused> {
    match w.read(|s| s.action(id).map(|a| a.state)) {
        Some(ActionState::Held) => Err(Refused::Rejected(
            "the decision could not be recorded".into(),
        )),
        _ => Ok(()),
    }
}

/// Approves a held merge: the approval is appended first, so the task is plainly being merged
/// while git works, then what the merge did.
fn merge(
    w: &Writer,
    id: ActionId,
    task: TaskId,
    decided: (Actor, ProjectEvent),
    tell: &dyn Fn(Option<&str>) -> (Actor, ProjectEvent),
) -> Result<(), Refused> {
    let clock = w.clock();
    let Some(plan) = w.read(|s| MergePlan::of(s, task)) else {
        let why = format!("task {task} is not waiting for a merge any more");
        w.append(vec![
            decided,
            gate::finished(id, Some(why.clone())),
            tell(Some(&why)),
        ])?;
        return Ok(());
    };
    w.append(vec![decided])?;
    let outcome = match w.read(git_of) {
        Some(git) => branches::merge(&git, plan.from, plan.into),
        None => Err("the project has no code directory to merge into".into()),
    };
    let mut tail = w.read(|s| tools::merged(s, clock, &plan, id, outcome));
    let failed = tail.iter().find_map(|(_, e)| match e {
        ProjectEvent::Task {
            event: TaskEvent::MergeFailed { conflict },
            ..
        } => Some(conflict.clone()),
        _ => None,
    });
    let result = match failed {
        None => "The merge is done.".to_owned(),
        Some(why) => format!("The merge failed ({why}); the task went back to its assignee."),
    };
    tail.push(tell(Some(&result)));
    w.append(tail)?;
    Ok(())
}

/// The task goes back to its assignee with `why`, so it is not left waiting for a merge.
fn sent_back(state: &ProjectState, clock: &dyn Clock, task: TaskId, why: &str) -> Events {
    let Some(assignee) = state
        .task(task)
        .and_then(|t| t.state.assignment())
        .map(|a| a.assignee)
    else {
        return Vec::new();
    };
    vec![
        (
            Actor::Daemon,
            ProjectEvent::Task {
                task,
                event: TaskEvent::MergeFailed {
                    conflict: why.to_owned(),
                },
            },
        ),
        deliver(clock, assignee, InboxItem::Task { task }),
    ]
}

/// Approved tool calls and merges a stopped daemon did not finish: each fails, a merge's task goes
/// back to its assignee, and the agent is told. So does a publish or deploy that was let through
/// and was still going in the background. Run when a project is attached. Approved commands and
/// requests need nothing: they wait, allowed once, for the agent's next try.
pub(crate) fn unfinished(state: &ProjectState, clock: &dyn Clock) -> Events {
    let mut out = Vec::new();
    for record in state.actions() {
        if record.state == ActionState::Allowed && background(&record.action.request) {
            let id = record.action.id;
            out.push(gate::finished(id, Some(NOT_CARRIED_OUT.into())));
            out.push(deliver(
                clock,
                record.action.agent,
                InboxItem::Finished {
                    action: id,
                    result: NOT_CARRIED_OUT.into(),
                },
            ));
            continue;
        }
        if record.state != ActionState::Approved {
            continue;
        }
        let id = record.action.id;
        match &record.action.request {
            Request::Tool { .. } => {}
            Request::Merge { task } => out.extend(sent_back(state, clock, *task, NOT_CARRIED_OUT)),
            Request::Command { .. } | Request::Egress { .. } => continue,
        }
        out.push(gate::finished(id, Some(NOT_CARRIED_OUT.into())));
        out.push(deliver(
            clock,
            record.action.agent,
            tell_agent(id, Decision::Approved, Some(NOT_CARRIED_OUT.into())),
        ));
    }
    out
}

/// A call whose work goes on after it returns, so an Allowed one may be left open by a stop: a
/// deploy, or a change on trial in the lab.
fn background(request: &Request) -> bool {
    use x100_core::tool::{Proposal, ToolCall};
    matches!(
        request,
        Request::Tool {
            call: ToolCall::Deploy { .. } | ToolCall::ProposeChange(Proposal::Trial { .. }),
            ..
        }
    )
}
