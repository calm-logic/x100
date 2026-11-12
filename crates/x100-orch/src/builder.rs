//! What the builder's requests do to a project's log: a message or a task reaching the CEO's
//! inbox, a raised task, a job waking an agent.

use x100_core::agent::AgentEvent;
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, FileId, InboxItemId, JobId, MessageId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::message::{Message, Route};
use x100_core::party::{Actor, Party};
use x100_core::task::{Assignment, Opening, Task};
use x100_core::time::Clock;
use x100_store::ProjectStore;

use crate::ids::new_id;
use crate::store::{append, append_all, read};

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

fn check_attachments(state: &ProjectState, files: &[FileId]) -> Result<(), Refused> {
    match files.iter().find(|f| state.file(**f).is_none()) {
        Some(f) => Err(Refused::NotFound(format!("no file {f}"))),
        None => Ok(()),
    }
}

/// The event that puts `what` in the agent's inbox. It is appended with the event that caused it,
/// so a crash cannot leave the cause in the log without the delivery.
fn delivery(clock: &dyn Clock, agent: AgentId, what: InboxItem) -> (Actor, ProjectEvent) {
    let item = new_id(clock, InboxItemId::new);
    (
        Actor::Daemon,
        ProjectEvent::Agent {
            agent,
            event: AgentEvent::Delivered { item, what },
        },
    )
}

/// The builder's message to the CEO, and its delivery to the CEO's inbox, in one append.
pub async fn send_chat(
    store: &ProjectStore,
    clock: &dyn Clock,
    body: String,
    attachments: Vec<FileId>,
    in_reply_to: Option<MessageId>,
) -> Result<MessageId, Refused> {
    let (ceo, known) = read(store, move |s| {
        let ceo = s.project().map(|p| p.ceo);
        let known = (
            check_attachments(s, &attachments).map(|()| attachments),
            in_reply_to.is_none_or(|m| s.message(m).is_some()),
        );
        (ceo, known)
    })
    .await;
    let ceo = ceo.ok_or_else(|| Refused::NotFound("the project has no CEO".into()))?;
    let attachments = known.0?;
    if !known.1 {
        return Err(Refused::NotFound("no such message to reply to".into()));
    }
    let id = new_id(clock, MessageId::new);
    let message = Message {
        id,
        route: Route::BuilderToCeo,
        body,
        attachments,
        in_reply_to,
    };
    append_all(
        store,
        vec![
            (Actor::Builder, ProjectEvent::MessageSent(message)),
            delivery(clock, ceo, InboxItem::Message { message: id }),
        ],
    )
    .await?;
    Ok(id)
}

/// The brief: the project's first message, and the task it delegates to the CEO.
pub async fn send_brief(
    store: &ProjectStore,
    clock: &dyn Clock,
    body: String,
    attachments: Vec<FileId>,
) -> Result<(MessageId, TaskId), Refused> {
    let (ceo, range, check, first) = read(store, {
        let attachments = attachments.clone();
        move |s| {
            let p = s.project();
            (
                p.map(|p| p.ceo),
                p.and_then(|p| p.team.effort(p.ceo)),
                check_attachments(s, &attachments),
                s.messages().next().is_none(),
            )
        }
    })
    .await;
    let ceo = ceo.ok_or_else(|| Refused::NotFound("the project has no CEO".into()))?;
    check?;
    if !first {
        return Err(Refused::WrongState(
            "the brief is the project's first message, and there is one already".into(),
        ));
    }
    let message = new_id(clock, MessageId::new);
    let task = new_id(clock, TaskId::new);
    let title: String = body
        .lines()
        .next()
        .unwrap_or("The brief")
        .chars()
        .take(80)
        .collect();
    let note = Message {
        id: message,
        route: Route::BuilderToCeo,
        body: body.clone(),
        attachments: attachments.clone(),
        in_reply_to: None,
    };
    let opening = Opening::Delegated(Assignment {
        delegator: Party::Builder,
        assignee: ceo,
        effort: clamp(range.unwrap_or(EffortRange::FULL), Effort::High),
    });
    let task_record = Task {
        id: task,
        title,
        ask: body,
        part: None,
        raised_by: Party::Builder,
        attachments,
    };
    append_all(
        store,
        vec![
            (Actor::Builder, ProjectEvent::MessageSent(note)),
            (
                Actor::Builder,
                ProjectEvent::TaskOpened {
                    task: task_record,
                    opening,
                },
            ),
            delivery(clock, ceo, InboxItem::Task { task }),
        ],
    )
    .await?;
    Ok((message, task))
}

fn clamp(range: EffortRange, wanted: Effort) -> Effort {
    wanted.clamp(range.min(), range.max())
}

/// A task raised as a new idea. It is nobody's until it is delegated.
pub async fn raise_task(
    store: &ProjectStore,
    clock: &dyn Clock,
    title: String,
    ask: String,
    part: Option<String>,
    attachments: Vec<FileId>,
) -> Result<TaskId, Refused> {
    let check = read(store, {
        let attachments = attachments.clone();
        move |s| check_attachments(s, &attachments)
    })
    .await;
    check?;
    let id = new_id(clock, TaskId::new);
    let task = Task {
        id,
        title,
        ask,
        part,
        raised_by: Party::Builder,
        attachments,
    };
    append(
        store,
        Actor::Builder,
        ProjectEvent::TaskOpened {
            task,
            opening: Opening::Raised,
        },
    )
    .await?;
    Ok(id)
}

/// A job that wakes an agent has fired: its note reaches the agent's inbox.
pub async fn deliver_job(
    store: &ProjectStore,
    clock: &dyn Clock,
    agent: AgentId,
    job: JobId,
    note: String,
) -> Result<(), Refused> {
    let (by, event) = delivery(clock, agent, InboxItem::Job { job, note });
    append(store, by, event).await?;
    Ok(())
}
