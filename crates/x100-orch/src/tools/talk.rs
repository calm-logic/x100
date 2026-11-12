//! `message`, `wait`, `schedule` and `cancel_job`.

use x100_core::agent::AgentEvent;
use x100_core::event::ProjectEvent;
use x100_core::id::{FileId, JobId, MessageId, TaskId, WaitId};
use x100_core::inbox::InboxItem;
use x100_core::job::{Job, JobEvent, JobState, ProjectJob, ProjectRoutine, Schedule};
use x100_core::message::{Message, Route};
use x100_core::party::{Actor, Party};
use x100_core::task::{Progress, TaskEvent};
use x100_core::time::Timestamp;
use x100_core::tool::{JobRequest, Recipient};
use x100_core::wait::{Wait, WaitOn};

use super::{At, Plan, Refusal, who};
use crate::ids::new_id;
use crate::settle::{already_over, deliver};

pub(super) fn message(
    at: &At,
    to: Recipient,
    body: &str,
    attachments: &[FileId],
    in_reply_to: Option<MessageId>,
) -> Result<Plan, Refusal> {
    let me = at.id();
    at.files_exist(attachments)?;
    if let Some(m) = in_reply_to
        && at.state.message(m).is_none()
    {
        return Err(format!("there is no message {m} to reply to"));
    }
    let route = match to {
        Recipient::Builder if at.is_ceo() => Route::CeoToBuilder,
        Recipient::Builder => {
            return Err(
                "only the CEO messages the builder: tell your parent, who passes it up".into(),
            );
        }
        Recipient::Agent(a) if a == me => return Err("an agent does not message itself".into()),
        Recipient::Agent(a) => {
            at.live_agent(a)?;
            Route::Between { from: me, to: a }
        }
    };
    let id = new_id(at.ctx.clock, MessageId::new);
    let mut events = vec![(
        Actor::Agent(me),
        ProjectEvent::MessageSent(Message {
            id,
            route,
            body: body.to_owned(),
            attachments: attachments.to_vec(),
            in_reply_to,
        }),
    )];
    if let Recipient::Agent(a) = to {
        events.push(deliver(at.ctx.clock, a, InboxItem::Message { message: id }));
    }
    Ok(Plan {
        events,
        ..Plan::default()
    })
}

pub(super) fn wait(
    at: &At,
    on: WaitOn,
    deadline: Option<Timestamp>,
    task: TaskId,
    progress: &Progress,
) -> Result<Plan, Refusal> {
    let me = at.id();
    at.held_task(task)?;
    check_waitable(at, on)?;
    let now = at.ctx.now;
    let deadline = deadline.unwrap_or(now + at.ctx.settings.default_wait_deadline);
    if deadline <= now {
        return Err("the deadline has passed already".into());
    }
    let id = new_id(at.ctx.clock, WaitId::new);
    let deadline_job = new_id(at.ctx.clock, JobId::new);
    let wait = Wait {
        id,
        on,
        deadline,
        deadline_job,
    };
    if let Some(end) = already_over(at.state, &wait) {
        return Err(format!(
            "there is nothing to wait for: that has {}",
            match end {
                x100_core::wait::WaitEnd::Satisfied => "happened already",
                _ => "become impossible",
            }
        ));
    }
    let job = Job {
        id: deadline_job,
        when: Schedule::Once { at: deadline },
        does: ProjectJob::Routine(ProjectRoutine::WaitDeadline {
            agent: me,
            wait: id,
        }),
    };
    let events = vec![
        (
            Actor::Agent(me),
            ProjectEvent::Task {
                task,
                event: TaskEvent::ProgressNoted(progress.clone()),
            },
        ),
        (Actor::Daemon, ProjectEvent::JobScheduled(job)),
        (
            Actor::Daemon,
            ProjectEvent::Agent {
                agent: me,
                event: AgentEvent::WaitOpened(wait),
            },
        ),
    ];
    Ok(Plan {
        events,
        ..Plan::default()
    })
}

/// Whether the caller may wait on `on`: a job of the project's that can still fire, the report on a
/// task it delegated, a reply to a message it sent, or the decision on an action it proposed.
fn check_waitable(at: &At, on: WaitOn) -> Result<(), Refusal> {
    let me = at.id();
    match on {
        WaitOn::Job(job) => match at.state.job(job) {
            None => Err(format!("there is no job {job}")),
            Some(j) if matches!(j.job.does, ProjectJob::Routine(_)) => {
                Err("the daemon's own routines are not waited on".into())
            }
            Some(_) => Ok(()),
        },
        WaitOn::Report(task) => match at.state.task(task).map(|t| &t.state) {
            None => Err(format!("there is no task {task}")),
            Some(s) if s.assignment().map(|a| a.delegator) != Some(Party::Agent(me)) => Err(
                format!("you did not delegate task {task}, so its report does not come to you"),
            ),
            Some(_) => Ok(()),
        },
        WaitOn::Reply { message, from } => {
            let m = at
                .state
                .message(message)
                .ok_or_else(|| format!("there is no message {message}"))?;
            let sent_to = match m.message.route {
                Route::Between { from: f, to } if f == me => Party::Agent(to),
                Route::CeoToBuilder if at.is_ceo() => Party::Builder,
                _ => return Err(format!("you did not send message {message}")),
            };
            if sent_to != from {
                return Err(format!(
                    "message {message} went to {}, so the reply comes from them",
                    who(at.state, sent_to)
                ));
            }
            Ok(())
        }
        WaitOn::Decision(action) => match at.state.action(action) {
            Some(a) if a.action.agent == me => Ok(()),
            Some(_) => Err(format!("action {action} is not yours")),
            None => Err(format!("there is no action {action}")),
        },
    }
}

pub(super) fn schedule(at: &At, when: Schedule, does: &JobRequest) -> Result<Plan, Refusal> {
    let me = at.id();
    if let Schedule::Every { every, .. } = when {
        let min = at.ctx.settings.minimum_repeat;
        if every.get() < min.get() {
            return Err(format!(
                "a job repeats at most every {:?}; {:?} is too often",
                min.get(),
                every.get()
            ));
        }
    }
    let does = match does.clone() {
        JobRequest::Wake { agent, note } => {
            at.live_agent(agent)?;
            if agent != me && !at.is_my_child(agent) {
                return Err(format!(
                    "{} is not one of your agents: an agent schedules jobs for itself and the \
                     agents it delegates to",
                    who(at.state, Party::Agent(agent))
                ));
            }
            ProjectJob::Wake {
                scheduled_by: me,
                agent,
                note,
            }
        }
        JobRequest::Delegate {
            to,
            title,
            ask,
            effort,
        } => {
            at.live_agent(to)?;
            if !at.is_my_child(to) {
                return Err(format!(
                    "{} is not one of your agents: a job delegates only to the agents you \
                     delegate to",
                    who(at.state, Party::Agent(to))
                ));
            }
            if title.trim().is_empty() {
                return Err("a task needs a title".into());
            }
            ProjectJob::Delegate {
                scheduled_by: me,
                to,
                title,
                ask,
                effort,
            }
        }
    };
    let job = Job {
        id: new_id(at.ctx.clock, JobId::new),
        when,
        does,
    };
    Ok(Plan {
        events: vec![(Actor::Agent(me), ProjectEvent::JobScheduled(job))],
        ..Plan::default()
    })
}

pub(super) fn cancel(at: &At, job: JobId) -> Result<Plan, Refusal> {
    let me = at.id();
    let rec = at
        .state
        .job(job)
        .ok_or_else(|| format!("there is no job {job}"))?;
    let mine = match &rec.job.does {
        ProjectJob::Wake { scheduled_by, .. } | ProjectJob::Delegate { scheduled_by, .. } => {
            *scheduled_by == me
        }
        ProjectJob::Routine(_) => false,
    };
    if !mine {
        return Err(format!(
            "you did not schedule job {job}: an agent cancels only its own"
        ));
    }
    if !matches!(rec.state, JobState::Scheduled { .. }) {
        return Err(format!("job {job} is {}, not scheduled", rec.state.name()));
    }
    let event = ProjectEvent::Job {
        job,
        event: JobEvent::Cancelled {
            by: Actor::Agent(me),
        },
    };
    Ok(Plan {
        events: vec![(Actor::Agent(me), event)],
        ..Plan::default()
    })
}
