//! Inbox items in words, for a turn's input and the start pack's list of what is unhandled.
//!
//! Every id an agent needs to act on an item is on a labelled line, `Task: tsk_...`, ahead of any
//! text someone else wrote, so a model reads it as prose and a script can look it up.

use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, FileId, InboxItemId, MessageId};
use x100_core::inbox::InboxItem;
use x100_core::message::Route;
use x100_core::party::Party;
use x100_core::task::{Outcome, TaskFailure};
use x100_core::wait::WaitEnd;

use crate::tools::who;

/// The text of one turn: what the given items say, then anything the daemon must add, then the
/// agent's current task; and the builder's message it answers, if any, so the CEO's reply can be
/// recorded as one.
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
    // A warm driver's start pack may name a task that is over, so each turn says which is current.
    // Its id only: the title would read as if the task had just arrived.
    if let Some(t) = state.current_task(agent) {
        parts.push(format!("Current task: {}", t.task.id));
    }
    (parts.join("\n\n---\n\n"), reply_to)
}

/// One inbox item in words.
pub fn describe(state: &ProjectState, item: &InboxItem) -> String {
    match item {
        InboxItem::Task { task } => match state.task(*task) {
            Some(t) => {
                let mut text = format!("A task for you: {}\nTask: {task}", t.task.title);
                if let Some(a) = t.state.assignment() {
                    text.push_str(&format!(
                        "\nDelegated by: {}\nEffort: {:?}",
                        party(state, a.delegator),
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
            None => format!("A task that is not in the log.\nTask: {task}"),
        },
        InboxItem::Message { message } => match state.message(*message) {
            Some(m) => {
                let from = match m.message.route {
                    Route::BuilderToCeo => Party::Builder,
                    Route::Between { from, .. } => Party::Agent(from),
                    Route::CeoToBuilder | Route::DaemonToBuilder => Party::Builder,
                };
                let mut text = format!(
                    "A message from {}.\nMessage: {message}\nFrom: {}",
                    who(state, from),
                    party(state, from)
                );
                if let Some(r) = m.message.in_reply_to {
                    text.push_str(&format!("\nIn reply to: {r}"));
                }
                // The task a builder's message opened, or the one it belongs to, so the CEO knows
                // what to report or wait on.
                if m.message.route == Route::BuilderToCeo
                    && let Some(ceo) = state.project().map(|p| p.ceo)
                    && let Some(task) = crate::builder::builder_task_open(state, ceo)
                {
                    text.push_str(&format!("\nTask for the builder: {task}"));
                }
                text.push_str(&format!(
                    "\n{}{}",
                    m.message.body,
                    attachments(state, &m.message.attachments)
                ));
                text
            }
            None => format!("A message that is not in the log.\nMessage: {message}"),
        },
        InboxItem::Report { task } => match state.task(*task) {
            Some(t) => {
                let from = t.state.assignment().map(|a| Party::Agent(a.assignee));
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
                let mut text = format!(
                    "A report on {}: {outcome}.\nReport on: {task}",
                    t.task.title
                );
                if let Some(from) = from {
                    text.push_str(&format!("\nFrom: {}", party(state, from)));
                }
                let summary = report.map(|r| r.summary.as_str()).unwrap_or_default();
                text.push_str(&format!(
                    "\n{summary}\nAccept it or send it back with `review`."
                ));
                text
            }
            None => format!("A report on a task that is not in the log.\nReport on: {task}"),
        },
        InboxItem::Decision { action, decision } => {
            format!("Your held action was {decision:?} by the builder.\nAction: {action}")
        }
        InboxItem::Job { job, note } => {
            format!("A scheduled job woke you: {note}\nJob: {job}")
        }
        InboxItem::WaitEnded { wait, end } => {
            let what = match end {
                WaitEnd::Expired => "ran out: its deadline passed before it was over",
                WaitEnd::CannotHappen => "ended: what it waited for can no longer happen",
                WaitEnd::Satisfied => "is over: what it waited for happened",
            };
            format!("Your wait {what}.\nWait: {wait}")
        }
        InboxItem::NoDriver { agent, why } => format!(
            "{} is waiting for a driver: {why:?}\nAgent: {agent}",
            who(state, Party::Agent(*agent))
        ),
        InboxItem::BudgetReached { lead } => format!(
            "{} reached its budget.\nLead: {lead}",
            who(state, Party::Agent(*lead))
        ),
        InboxItem::Nudge { task, turns } => format!(
            "Your last {turns} turn(s) ended with your task neither reported nor waited on. If \
             its work is done, `report` it now. If it needs something first, `wait` for that. \
             Otherwise carry on, and end this turn with one of the two.\nTask: {task}"
        ),
    }
}

/// A party as the value of a labelled line: its id first, so a lookup reads the id, or `builder`.
pub(crate) fn party(state: &ProjectState, party: Party) -> String {
    match party {
        Party::Builder => "builder".into(),
        Party::Agent(a) => match state.agent(a) {
            Some(r) => format!("{a} ({})", r.role),
            None => a.to_string(),
        },
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
