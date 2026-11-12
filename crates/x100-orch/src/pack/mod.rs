//! What a driver is told: the start pack it starts from, and the text of each turn.
//!
//! Every driver starts from a start pack built from the daemon's records, never from a provider's
//! own memory of the work, so a fresh task, a wake after a long wait and a new driver after a
//! failure all start the same way and differ only in what the records hold. A guide to the agent's
//! position and to what its driver may do comes first, since nothing else tells a model how x100
//! works; the sections after it follow ARCHITECTURE "What lives outside a driver". Memories, skills
//! and running processes arrive with later milestones and have no section yet.
//!
//! The pack takes at most the daemon's set share of the model's context window. The transcript's
//! older part is what gets cut, which is why the progress note matters.

mod guide;
mod history;
mod items;

use std::fmt::Write as _;

use x100_core::agent::Position;
use x100_core::driver::{DriverState, EndReason, StartReason};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId};
use x100_core::job::{JobState, ProjectJob};
use x100_core::message::Route;
use x100_core::party::Party;
use x100_core::task::TaskState;
use x100_core::transcript::{Entry, Item};
use x100_core::wait::WaitOn;
use x100_store::ProjectStore;

pub use items::{describe, turn_input};

use items::party;

use crate::branches;
use crate::tools::who;

/// Roughly how many characters a token is, for keeping the pack inside its share of the window.
const CHARS_PER_TOKEN: u64 = 4;
const TRANSCRIPT_ENTRIES: u64 = 400;
const RESULT_CHARS: usize = 400;
/// How many of its finished tasks an agent's pack recalls, with their reports and last notes. A
/// retired driver's context is gone, and this is what the next one needs of it until memories.
const RECENT_TASKS: usize = 5;

/// What a start pack is built for, besides the agent's records.
pub(crate) struct Pack {
    pub why: StartReason,
    /// What the driver may do in its working directory, which depends on its harness.
    pub workspace: String,
    pub window_tokens: u64,
    pub share_percent: u8,
}

/// Builds an agent's start pack from its records. Blocks on the store and git.
pub(crate) fn build(store: &ProjectStore, agent: AgentId, for_: &Pack) -> String {
    let (window_tokens, share_percent) = (for_.window_tokens, for_.share_percent);
    let state = store.log.state();
    let task = state.current_task(agent).map(|t| t.task.id);
    let history = task
        .map(|t| history::history(store, agent, t))
        .unwrap_or_default();
    let diff = history
        .base
        .as_ref()
        .and_then(|base| branches::diff_since(store, agent, base));
    let mut pack = head(&state, agent, for_);
    if !history.calls.is_empty() {
        pack.push_str("\nDAEMON TOOL CALLS IN THIS TASK (each took effect; do not repeat them):\n");
        for call in &history.calls {
            pack.push_str(&call.line());
            pack.push('\n');
        }
    }
    if let Some(diff) = diff {
        let _ = write!(pack, "\nYOUR WORK SINCE THE TASK BEGAN:\n{diff}\n");
    }
    let budget = window_tokens.saturating_mul(u64::from(share_percent)) / 100 * CHARS_PER_TOKEN;
    let room = usize::try_from(budget)
        .unwrap_or(usize::MAX)
        .saturating_sub(pack.len());
    let tail = transcript_tail(store, agent, room);
    if !tail.is_empty() {
        let _ = write!(pack, "\nYOUR TRANSCRIPT, LATEST LAST:\n{tail}");
    }
    pack
}

/// Everything but the calls, the diff and the transcript.
fn head(state: &ProjectState, agent: AgentId, for_: &Pack) -> String {
    let Some(a) = state.agent(agent) else {
        return String::new();
    };
    let mut p = String::new();
    let _ = writeln!(p, "ROLE: {}", a.role);
    let _ = writeln!(p, "Your id: {agent}");
    if let Some(project) = state.project() {
        let team = &project.team;
        let (position, place) = match team.position(agent) {
            Some(Position::Ceo { .. }) => (
                guide::Position::Ceo,
                "You are the CEO, and you answer to the builder.".to_owned(),
            ),
            Some(Position::Lead { budget, .. }) => (
                guide::Position::Lead,
                format!(
                    "You are {}, a lead under the CEO, with a budget of {} micro-dollars.",
                    a.role,
                    budget.micros()
                ),
            ),
            Some(Position::Worker { parent, .. }) => (
                guide::Position::Worker,
                format!(
                    "You are {}, a worker under {}.",
                    a.role,
                    who(state, Party::Agent(*parent))
                ),
            ),
            None => (
                guide::Position::Worker,
                format!("You are {}, no longer on the team.", a.role),
            ),
        };
        let _ = write!(p, "\nHOW YOU WORK:\n{}", guide::guide(position));
        let _ = write!(p, "\nYOUR WORKING DIRECTORY:\n{}\n", for_.workspace);
        let _ = write!(p, "\nYOU:\n{place}\n");
        if let Some(range) = team.effort(agent) {
            let _ = writeln!(
                p,
                "Your effort range: {:?} to {:?}.",
                range.min(),
                range.max()
            );
        }
        let _ = write!(p, "\nCHARTER:\n{}\n", a.charter);
        let children = state.children(agent);
        if !children.is_empty() {
            p.push_str("\nYOUR AGENTS (delegate to them, or start new workers):\n");
            for c in children {
                p.push_str(&member(state, c));
            }
        }
        let leads: Vec<_> = team
            .members()
            .filter(|(id, pos)| {
                *id != agent && matches!(pos, Position::Lead { .. }) && !team.is_retired(*id)
            })
            .map(|(id, _)| id)
            .collect();
        if matches!(team.position(agent), Some(Position::Lead { .. })) && !leads.is_empty() {
            p.push_str("\nTHE OTHER LEADS (message them directly):\n");
            for l in leads {
                p.push_str(&member(state, l));
            }
        }
    }
    if let Some(t) = state.current_task(agent) {
        let _ = write!(
            p,
            "\nCURRENT TASK:\nTask: {} ({})\n",
            t.task.id, t.task.title
        );
        if let Some(a) = t.state.assignment() {
            let _ = writeln!(p, "Delegated by: {}", party(state, a.delegator));
            let _ = writeln!(p, "Effort: {:?}", a.effort);
        }
        let _ = writeln!(p, "{}", t.task.ask);
        match &t.progress {
            Some(pr) => {
                let done = pr
                    .done
                    .map(|d| format!(" ({}% done)", d.value()))
                    .unwrap_or_default();
                let _ = writeln!(p, "Progress note{done}: {}", pr.note);
            }
            None => p.push_str("No progress note yet.\n"),
        }
    }
    let others: Vec<_> = a.open_tasks.iter().rev().skip(1).collect();
    if !others.is_empty() {
        p.push_str("\nYOUR OTHER OPEN TASKS:\n");
        for t in others.into_iter().filter_map(|t| state.task(*t)) {
            let note = t
                .progress
                .as_ref()
                .map(|p| p.note.as_str())
                .unwrap_or("no note");
            let _ = writeln!(p, "- {}: {} ({note})", t.task.id, t.task.title);
        }
    }
    let recent = recent_tasks(state, agent);
    if !recent.is_empty() {
        p.push_str("\nYOUR RECENT TASKS, LATEST LAST (what you reported, and your last note):\n");
        p.push_str(&recent);
    }
    let business = open_business(state, agent);
    if !business.is_empty() {
        let _ = write!(p, "\nOPEN BUSINESS:\n{business}");
    }
    if !a.inbox.is_empty() {
        p.push_str("\nYOUR INBOX, NOT YET HANDLED:\n");
        for id in a.inbox.unhandled() {
            if let Some(item) = a.items.get(&id) {
                let _ = writeln!(p, "- {}", describe(state, item).replace('\n', "\n  "));
            }
        }
    }
    let _ = write!(
        p,
        "\nWHY THIS DRIVER IS STARTING:\n{}\n",
        reason(state, agent, &for_.why)
    );
    p
}

/// The tasks `agent` reported most recently, each with its report and the last progress note.
fn recent_tasks(state: &ProjectState, agent: AgentId) -> String {
    let mut done: Vec<_> = state
        .tasks()
        .filter_map(|t| {
            let (asg, report) = match &t.state {
                TaskState::InReview(a, r) | TaskState::Promoting(a, r) | TaskState::Done(a, r) => {
                    (a, r)
                }
                _ => return None,
            };
            (asg.assignee == agent).then_some((t, report))
        })
        .collect();
    done.sort_by_key(|(t, _)| (t.opened, t.task.id));
    let skip = done.len().saturating_sub(RECENT_TASKS);
    let mut out = String::new();
    for (t, report) in done.into_iter().skip(skip) {
        let outcome = match &report.outcome {
            x100_core::task::Outcome::Completed => "completed".to_owned(),
            x100_core::task::Outcome::Failed(f) => format!("failed: {}", items::failure(f)),
        };
        let _ = writeln!(
            out,
            "- {} \"{}\", {}, {outcome}. Report: {}",
            t.task.id,
            t.task.title,
            t.state.name(),
            report.summary
        );
        if let Some(note) = &t.progress {
            let _ = writeln!(out, "  Last note: {}", note.note);
        }
    }
    out
}

/// A member of the team as a labelled line: `Lead Tech: agt_...` or `Worker Backend: agt_...`.
fn member(state: &ProjectState, agent: AgentId) -> String {
    let position = match state.project().and_then(|p| p.team.position(agent)) {
        Some(Position::Ceo { .. }) => "CEO",
        Some(Position::Lead { .. }) => "Lead",
        _ => "Worker",
    };
    let role = state.agent(agent).map_or("", |a| a.role.as_str());
    format!("{position} {role}: {agent}\n")
}

/// Waits, jobs it scheduled, tasks it delegated and has not accepted, and messages awaiting a
/// reply. Held actions arrive with the gate.
fn open_business(state: &ProjectState, agent: AgentId) -> String {
    let Some(a) = state.agent(agent) else {
        return String::new();
    };
    let mut b = String::new();
    for w in a.waits.values() {
        let on = match w.on {
            WaitOn::Job(j) => format!("job {j}"),
            WaitOn::Report(t) => format!("the report on task {t}"),
            WaitOn::Reply { message, from } => {
                format!("{}'s reply to message {message}", who(state, from))
            }
            WaitOn::Decision(action) => format!("the decision on action {action}"),
        };
        let _ = writeln!(
            b,
            "- Waiting ({}) on {on}, until {}",
            w.id,
            w.deadline.as_millis()
        );
    }
    for j in state.jobs_of(agent) {
        let due = match j.state {
            JobState::Scheduled { due, .. } => due.as_millis(),
            _ => continue,
        };
        let what = match &j.job.does {
            ProjectJob::Wake {
                agent: to, note, ..
            } => {
                format!("wakes {} with \"{note}\"", who(state, Party::Agent(*to)))
            }
            ProjectJob::Delegate { to, title, .. } => {
                format!("delegates \"{title}\" to {}", who(state, Party::Agent(*to)))
            }
            ProjectJob::Routine(_) => continue,
        };
        let _ = writeln!(b, "- Job {} {what}, next at {due}", j.job.id);
    }
    for t in state.tasks() {
        let Some(asg) = t.state.assignment() else {
            continue;
        };
        if asg.delegator != Party::Agent(agent) || matches!(t.state, TaskState::Done(..)) {
            continue;
        }
        let _ = writeln!(
            b,
            "- Task {} \"{}\" delegated to {}: {}",
            t.task.id,
            t.task.title,
            who(state, Party::Agent(asg.assignee)),
            t.state.name()
        );
    }
    for m in state.messages() {
        let mine = match m.message.route {
            Route::Between { from, .. } => from == agent,
            Route::CeoToBuilder => state.project().is_some_and(|p| p.ceo == agent),
            _ => false,
        };
        if mine && m.awaited && m.reply.is_none() {
            let _ = writeln!(
                b,
                "- Message {} awaits a reply: {}",
                m.message.id, m.message.body
            );
        }
    }
    b
}

fn reason(state: &ProjectState, agent: AgentId, why: &StartReason) -> String {
    let mut text = match why {
        StartReason::NewTask { task } => format!("A new task, {task}, reached you."),
        StartReason::Wake => "Something reached your inbox.".to_owned(),
        StartReason::Handoff(h) => format!("You take over from {}: {:?}.", h.replaces, h.cause),
    };
    // The driver being started is in the record already, and live.
    let previous = state.agent(agent).and_then(|a| {
        a.drivers
            .iter()
            .rev()
            .filter_map(|d| state.driver(*d))
            .find(|d| !d.state.is_live())
    });
    if let Some(d) = previous {
        let ended = match &d.state {
            DriverState::Failed(f) => format!("failed ({f:?}); what it was doing is lost"),
            DriverState::Ended(EndReason::Retired(r)) => format!("retired ({r:?})"),
            DriverState::Ended(EndReason::Stopped(s)) => format!("was stopped ({s:?})"),
            DriverState::Ended(other) => format!("ended ({other:?})"),
            _ => "ended".to_owned(),
        };
        let _ = write!(
            text,
            " Your previous driver, {}, {ended}. Carry on from the progress note and the records \
             above.",
            d.driver.id
        );
    }
    text
}

/// The start pack `driver` was given: the first entry it wrote in its agent's transcript.
pub(crate) fn recorded(store: &ProjectStore, driver: DriverId) -> Option<String> {
    let agent = store
        .log
        .with_state(|s| s.driver(driver).map(|d| d.driver.agent))?;
    let mut after = None;
    loop {
        let page = store
            .transcripts
            .read(agent, after, TRANSCRIPT_ENTRIES as usize)
            .ok()?;
        let (last, _) = page.last()?;
        after = Some(*last);
        let found = page.into_iter().find_map(|(_, e)| match e.item {
            Item::StartPack { text } if e.driver == driver => Some(text),
            _ => None,
        });
        if found.is_some() {
            return found;
        }
    }
}

/// As much of the transcript's end as fits in `room` characters, oldest first.
fn transcript_tail(store: &ProjectStore, agent: AgentId, room: usize) -> String {
    let Ok(Some(last)) = store.transcripts.last(agent) else {
        return String::new();
    };
    let after = last
        .0
        .checked_sub(TRANSCRIPT_ENTRIES)
        .map(x100_core::transcript::Pos);
    let Ok(entries) = store
        .transcripts
        .read(agent, after, TRANSCRIPT_ENTRIES as usize)
    else {
        return String::new();
    };
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    for entry in entries.iter().rev() {
        let Some(line) = entry_line(&entry.1) else {
            continue;
        };
        if used + line.len() + 1 > room {
            break;
        }
        used += line.len() + 1;
        lines.push(line);
    }
    lines.reverse();
    lines.join("\n")
}

fn entry_line(entry: &Entry) -> Option<String> {
    let clip = |s: &str| {
        if s.len() > RESULT_CHARS {
            let mut end = RESULT_CHARS;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &s[..end])
        } else {
            s.to_owned()
        }
    };
    Some(match &entry.item {
        Item::TurnStarted => "[turn]".to_owned(),
        Item::Text { text } => text.clone(),
        Item::ToolCall { name, input, .. } => format!("> {name} {}", clip(&input.to_string())),
        Item::ToolResult {
            output, is_error, ..
        } => {
            let mark = if *is_error { "< error: " } else { "< " };
            format!("{mark}{}", clip(output))
        }
        Item::TurnEnded { how } => format!("[turn ended: {how:?}]"),
        Item::DriverChanged { .. } => format!("[driver {}]", entry.driver),
        // A pack the agent was given before is already in what this one says.
        Item::StartPack { .. } | Item::ToolOutput { .. } | Item::Usage { .. } => return None,
    })
}
