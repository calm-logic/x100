//! What the builder's requests do to a project's log. There is one chat per project, with the
//! CEO: the builder's messages go to the CEO only.
//!
//! The builder delegates to the CEO. The brief opens a task, and a later message opens one only
//! when the CEO has none in progress from the builder, so the CEO always works under a task with a
//! progress note. Such a message is still delivered as the message it is: the CEO answers the
//! builder, and finds the task it opened in its records.

use x100_core::effort::{Effort, EffortRange};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, FileId, JobId, MessageId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::job::{JobEvent, JobState};
use x100_core::message::{Message, Route};
use x100_core::party::{Actor, Party};
use x100_core::task::{Assignment, Opening, Task, TaskState};
use x100_core::time::Clock;

use crate::ctx::{Events, ProjectCtx};
use crate::ids::new_id;
use crate::settle::deliver;

/// Why a request was refused.
#[derive(Debug, thiserror::Error)]
pub enum Refused {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    WrongState(String),
    #[error(transparent)]
    Store(#[from] x100_store::Error),
}

type Decided<T> = (Events, Result<T, Refused>);

fn refused<T>(e: Refused) -> Decided<T> {
    (Vec::new(), Err(e))
}

fn check_attachments(state: &ProjectState, files: &[FileId]) -> Result<(), Refused> {
    match files.iter().find(|f| state.file(**f).is_none()) {
        Some(f) => Err(Refused::NotFound(format!("no file {f}"))),
        None => Ok(()),
    }
}

fn ceo(state: &ProjectState) -> Result<AgentId, Refused> {
    state
        .project()
        .map(|p| p.ceo)
        .ok_or_else(|| Refused::NotFound("the project has no CEO".into()))
}

/// A task the builder delegates to the CEO, titled by the first line of what it asks.
fn builder_task(
    state: &ProjectState,
    clock: &dyn Clock,
    ceo: AgentId,
    body: &str,
    attachments: Vec<FileId>,
) -> (TaskId, (Actor, ProjectEvent)) {
    let id = new_id(clock, TaskId::new);
    let title: String = body
        .lines()
        .next()
        .unwrap_or("The brief")
        .chars()
        .take(80)
        .collect();
    let range = state
        .project()
        .and_then(|p| p.team.effort(ceo))
        .unwrap_or(EffortRange::FULL);
    let task = Task {
        id,
        title,
        ask: body.to_owned(),
        part: None,
        raised_by: Party::Builder,
        attachments,
    };
    let opening = Opening::Delegated(Assignment {
        delegator: Party::Builder,
        assignee: ceo,
        effort: range.clamp(Effort::High),
    });
    (
        id,
        (Actor::Builder, ProjectEvent::TaskOpened { task, opening }),
    )
}

/// The task the CEO has in progress for the builder, if any.
pub(crate) fn builder_task_open(state: &ProjectState, ceo: AgentId) -> Option<TaskId> {
    state.tasks().find_map(|t| match &t.state {
        TaskState::InProgress(a) if a.delegator == Party::Builder && a.assignee == ceo => {
            Some(t.task.id)
        }
        _ => None,
    })
}

/// The builder's message to the CEO, delivered to its inbox, with a task when it has none open
/// from the builder. Blocks.
pub(crate) fn send_chat(
    ctx: &ProjectCtx,
    body: String,
    attachments: Vec<FileId>,
    in_reply_to: Option<MessageId>,
) -> Result<MessageId, Refused> {
    ctx.commit(|s, clock| -> Decided<MessageId> {
        let ceo = match ceo(s) {
            Ok(c) => c,
            Err(e) => return refused(e),
        };
        if let Err(e) = check_attachments(s, &attachments) {
            return refused(e);
        }
        if in_reply_to.is_some_and(|m| s.message(m).is_none()) {
            return refused(Refused::NotFound("no such message to reply to".into()));
        }
        let id = new_id(clock, MessageId::new);
        let message = Message {
            id,
            route: Route::BuilderToCeo,
            body: body.clone(),
            attachments: attachments.clone(),
            in_reply_to,
        };
        let mut events = vec![(Actor::Builder, ProjectEvent::MessageSent(message))];
        if builder_task_open(s, ceo).is_none() {
            let (_, opened) = builder_task(s, clock, ceo, &body, attachments.clone());
            events.push(opened);
        }
        events.push(deliver(clock, ceo, InboxItem::Message { message: id }));
        (events, Ok(id))
    })?
}

/// The brief: the project's first message, and the task it delegates to the CEO. Blocks.
pub(crate) fn send_brief(
    ctx: &ProjectCtx,
    body: String,
    attachments: Vec<FileId>,
) -> Result<(MessageId, TaskId), Refused> {
    ctx.commit(|s, clock| -> Decided<(MessageId, TaskId)> {
        let ceo = match ceo(s) {
            Ok(c) => c,
            Err(e) => return refused(e),
        };
        if let Err(e) = check_attachments(s, &attachments) {
            return refused(e);
        }
        if s.messages().next().is_some() {
            return refused(Refused::WrongState(
                "the brief is the project's first message, and there is one already".into(),
            ));
        }
        let message = new_id(clock, MessageId::new);
        let note = Message {
            id: message,
            route: Route::BuilderToCeo,
            body: body.clone(),
            attachments: attachments.clone(),
            in_reply_to: None,
        };
        let (task, opened) = builder_task(s, clock, ceo, &body, attachments.clone());
        let events = vec![
            (Actor::Builder, ProjectEvent::MessageSent(note)),
            opened,
            deliver(clock, ceo, InboxItem::Task { task }),
        ];
        (events, Ok((message, task)))
    })?
}

/// A task raised as a new idea. It is nobody's until it is delegated. Blocks.
pub(crate) fn raise_task(
    ctx: &ProjectCtx,
    title: String,
    ask: String,
    part: Option<String>,
    attachments: Vec<FileId>,
) -> Result<TaskId, Refused> {
    ctx.commit(|s, clock| -> Decided<TaskId> {
        if let Err(e) = check_attachments(s, &attachments) {
            return refused(e);
        }
        let id = new_id(clock, TaskId::new);
        let task = Task {
            id,
            title: title.clone(),
            ask: ask.clone(),
            part: part.clone(),
            raised_by: Party::Builder,
            attachments: attachments.clone(),
        };
        let event = ProjectEvent::TaskOpened {
            task,
            opening: Opening::Raised,
        };
        (vec![(Actor::Builder, event)], Ok(id))
    })?
}

/// The builder cancels a job an agent scheduled. Waits on it end, as it can no longer fire.
/// Blocks.
pub(crate) fn cancel_job(ctx: &ProjectCtx, job: JobId) -> Result<(), Refused> {
    ctx.commit(|s, _| -> Decided<()> {
        match s.job(job).map(|j| j.state) {
            None => refused(Refused::NotFound(format!("no job {job}"))),
            Some(JobState::Scheduled { .. }) => {
                let event = ProjectEvent::Job {
                    job,
                    event: JobEvent::Cancelled { by: Actor::Builder },
                };
                (vec![(Actor::Builder, event)], Ok(()))
            }
            Some(other) => refused(Refused::WrongState(format!(
                "job {job} is {}, not scheduled",
                other.name()
            ))),
        }
    })?
}
