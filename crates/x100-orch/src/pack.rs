//! What a driver is told: its first instructions and the text of each turn, built from the log.
//!
//! M5 gives the minimum a driver needs to act as its role. M6 replaces [`start_pack`] with the full
//! start pack of `ARCHITECTURE.md`.

use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, InboxItemId, MessageId};
use x100_core::inbox::InboxItem;
use x100_core::message::Route;

/// The first thing a driver reads. The fake harness's scripts match on `ROLE: CEO`.
pub fn start_pack(state: &ProjectState, agent: AgentId) -> String {
    let Some(a) = state.agent(agent) else {
        return String::new();
    };
    format!(
        "ROLE: {}\n\nCHARTER:\n{}\n\nWhat you write in a turn is your reply to whoever the turn is from.\n",
        a.role, a.charter
    )
}

/// The text of one turn: what the given inbox items say, and the builder's message it answers, if
/// any, so the reply can be recorded as one.
pub fn turn_input(
    state: &ProjectState,
    agent: AgentId,
    items: &[InboxItemId],
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
        parts.push(match item {
            InboxItem::Message { message } => match state.message(*message) {
                Some(m) => {
                    if m.message.route == Route::BuilderToCeo {
                        reply_to = Some(*message);
                    }
                    let from = match m.message.route {
                        Route::BuilderToCeo => "the builder".to_owned(),
                        Route::Between { from, .. } => state
                            .agent(from)
                            .map_or_else(|| from.to_string(), |a| a.role.clone()),
                        _ => "the daemon".to_owned(),
                    };
                    format!(
                        "Message from {from}:\n{}{}",
                        m.message.body,
                        attachments(state, &m.message.attachments)
                    )
                }
                None => format!("A message ({message}) that is not in the log."),
            },
            InboxItem::Task { task } => match state.task(*task) {
                Some(t) => format!(
                    "Task: {}\n{}{}",
                    t.task.title,
                    t.task.ask,
                    attachments(state, &t.task.attachments)
                ),
                None => format!("A task ({task}) that is not in the log."),
            },
            InboxItem::Job { note, .. } => format!("A scheduled job woke you: {note}"),
            other => format!("Inbox: {other:?}"),
        });
    }
    (parts.join("\n\n---\n\n"), reply_to)
}

fn attachments(state: &ProjectState, files: &[x100_core::id::FileId]) -> String {
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
