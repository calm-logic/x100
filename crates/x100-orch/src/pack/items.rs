//! Inbox items in words, for a turn's input and the start pack's list of what is unhandled.

use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, FileId, InboxItemId, MessageId};
use x100_core::inbox::InboxItem;
use x100_core::message::Route;
use x100_core::party::Party;
use x100_core::task::{Outcome, TaskFailure};
use x100_core::wait::WaitEnd;

use crate::tools::who;

/// The text of one turn: what the given items say, then anything the daemon must add, and the
/// builder's message it answers, if any, so the CEO's reply can be recorded as one.
pub fn turn_input(
    state: &ProjectState,
    agent: AgentId,
    items: &[InboxItemId],
    notes: &[String],
) -> (String, Option<MessageId>) {
    let Some(a) = state.agent(agent) else {
        return (String::new(), None);
    };
    let mut parts = Vec::new();
    let mut reply_to = None;
    for id in items {
        let Some(item) = a.items.get(id) else {
            continue;
        };
        if let InboxItem::Message { message } = item
            && state
                .message(*message)
                .is_some_and(|m| m.message.route == Route::BuilderToCeo)
        {
            reply_to = Some(*message);
        }
        parts.push(describe(state, item));
    }
    parts.extend(notes.iter().cloned());
    (parts.join("\n\n---\n\n"), reply_to)
}

/// One inbox item in words.
pub fn describe(state: &ProjectState, item: &InboxItem) -> String {
    match item {
        InboxItem::Task { task } => match state.task(*task) {
            Some(t) => {
                let mut text = format!("Task {task}: {}", t.task.title);
                if let Some(a) = t.state.assignment() {
                    text.push_str(&format!(
                        "\nFrom {}, at {:?} effort.",
                        who(state, a.delegator),
                        a.effort
                    ));
                }
                text.push_str(&format!(
                    "\n{}{}",
                    t.task.ask,
                    attachments(state, &t.task.attachments)
                ));
                if let Some(note) = &t.sent_back {
                    text.push_str(&format!("\nIt was sent back to you: {note}"));
                }
                if let Some(conflict) = &t.conflict {
                    text.push_str(&format!(
                        "\nIts merge failed, so it is back with you: {conflict}"
                    ));
                }
                text
            }
            None => format!("A task ({task}) that is not in the log."),
        },
        InboxItem::Message { message } => match state.message(*message) {
            Some(m) => {
                let from = match m.message.route {
                    Route::BuilderToCeo => "the builder".to_owned(),
                    Route::Between { from, .. } => who(state, Party::Agent(from)),
                    Route::CeoToBuilder | Route::DaemonToBuilder => "the daemon".to_owned(),
                };
                let answers = m
                    .message
                    .in_reply_to
                    .map(|r| format!(" in reply to {r}"))
                    .unwrap_or_default();
                let mut text = format!(
                    "Message {message} from {from}{answers}:\n{}{}",
                    m.message.body,
                    attachments(state, &m.message.attachments)
                );
                // The task a builder's message opened, or the one it belongs to, so the CEO knows
                // what to report or wait on.
                if m.message.route == Route::BuilderToCeo
                    && let Some(ceo) = state.project().map(|p| p.ceo)
                    && let Some(task) = crate::builder::builder_task_open(state, ceo)
                {
                    text.push_str(&format!("\nYour task for the builder: {task}"));
                }
                text
            }
            None => format!("A message ({message}) that is not in the log."),
        },
        InboxItem::Report { task } => match state.task(*task) {
            Some(t) => {
                let from = t
                    .state
                    .assignment()
                    .map(|a| who(state, Party::Agent(a.assignee)))
                    .unwrap_or_default();
                let report = match &t.state {
                    x100_core::task::TaskState::InReview(_, r)
                    | x100_core::task::TaskState::Promoting(_, r)
                    | x100_core::task::TaskState::Done(_, r) => Some(r),
                    _ => None,
                };
                let outcome =
                    report.map_or("no longer in review".to_owned(), |r| match &r.outcome {
                        Outcome::Completed => "completed".to_owned(),
                        Outcome::Failed(f) => format!("failed: {}", failure(f)),
                    });
                let summary = report.map(|r| r.summary.as_str()).unwrap_or_default();
                format!(
                    "Report on task {task} ({}) from {from}: {outcome}.\n{summary}\nAccept it or \
                     send it back with `review`.",
                    t.task.title
                )
            }
            None => format!("A report on a task ({task}) that is not in the log."),
        },
        InboxItem::Decision { action, decision } => {
            format!("Your held action {action} was {decision:?} by the builder.")
        }
        InboxItem::Job { note, .. } => format!("A scheduled job woke you: {note}"),
        InboxItem::WaitEnded { wait, end } => match end {
            WaitEnd::Expired => {
                format!("Your wait {wait} ran out: its deadline passed before it was over.")
            }
            WaitEnd::CannotHappen => {
                format!("Your wait {wait} ended: what it waited for can no longer happen.")
            }
            WaitEnd::Satisfied => format!("Your wait {wait} is over: what it waited for happened."),
        },
        InboxItem::NoDriver { agent, why } => format!(
            "{} is waiting for a driver: {why:?}",
            who(state, Party::Agent(*agent))
        ),
        InboxItem::BudgetReached { lead } => {
            format!("{} reached its budget.", who(state, Party::Agent(*lead)))
        }
        InboxItem::Nudge { task, turns } => format!(
            "Your last {turns} turn(s) ended with task {task} neither reported nor waited on. \
             Carry on, wait for what it needs with `wait`, or `report` it."
        ),
    }
}

pub(crate) fn failure(f: &TaskFailure) -> String {
    match f {
        TaskFailure::Assignee { reason } => reason.clone(),
        TaskFailure::DriverFailures(fs) => {
            format!("its drivers failed {} times in a row", fs.len())
        }
        TaskFailure::NoDriver(why) => format!("no driver could be found: {why:?}"),
        TaskFailure::NoProgress { turns } => {
            format!("{turns} turns in a row ended with it neither reported nor waited on")
        }
    }
}

fn attachments(state: &ProjectState, files: &[FileId]) -> String {
    let names: Vec<_> = files
        .iter()
        .filter_map(|f| state.file(*f).map(|f| f.file.name.clone()))
        .collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("\nAttached: {}", names.join(", "))
    }
}
