//! What a driver is told: the start pack it starts from, and the text of each turn.
//!
//! Every driver starts from a start pack built from the daemon's records, never from a provider's
//! own memory of the work, so a fresh task, a wake after a long wait and a new driver after a
//! failure all start the same way and differ only in what the records hold. A guide to the agent's
//! position and to what its driver may do comes first, since nothing else tells a model how x100
//! works; the sections after it follow ARCHITECTURE "What lives outside a driver". Memory and
//! skills come from `x100-memory`: the agent's index of notes and a list of the skills it can
//! load, each cut to a fixed size however much exists, so they add a bounded amount to the pack.
//! The notes and the skills' texts are read with `read_note` and `load_skill`, not carried.
//!
//! The pack takes at most the daemon's set share of the model's context window. The transcript's
//! older part is what gets cut, which is why the progress note matters.

mod guide;
mod history;
mod items;
mod why;

use std::fmt::Write as _;

use x100_core::agent::Position;
use x100_core::driver::StartReason;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId};
use x100_core::job::{JobState, ProjectJob};
use x100_core::message::Route;
use x100_core::party::Party;
use x100_core::task::TaskState;
use x100_core::transcript::{Entry, Item};
use x100_core::wait::WaitOn;
use x100_store::{Git, ProjectStore};

pub use items::{describe, turn_input};
pub(crate) use why::last_turn;

use items::party;

use crate::branches;
use crate::tools::who;

/// Roughly how many characters a token is, for keeping the pack inside its share of the window.
const CHARS_PER_TOKEN: u64 = 4;
const TRANSCRIPT_ENTRIES: u64 = 400;
const RESULT_CHARS: usize = 400;
/// How many of its finished tasks an agent's pack recalls, with their reports and last notes. A
/// retired driver's context is gone; this and the agent's notes are what the next one has of it.
const RECENT_TASKS: usize = 5;
/// How many of the latest chat messages a CEO's pack recalls, and how much of each.
const CHAT_MESSAGES: usize = 20;
const CHAT_CHARS: usize = 2_000;

/// What a start pack is built for, besides the agent's records.
pub(crate) struct Pack {
    pub why: StartReason,
    /// What the driver may do in its working directory, which depends on its harness.
    pub workspace: String,
    pub window_tokens: u64,
    pub share_percent: u8,
}

/// Builds an agent's start pack from its records. Blocks on the store and git. `git` is the
/// project's code directory, which a project that is not set up may not have.
pub(crate) fn build(
    store: &ProjectStore,
    git: Option<&Git>,
    agent: AgentId,
    for_: &Pack,
) -> String {
    let (window_tokens, share_percent) = (for_.window_tokens, for_.share_percent);
    let state = store.log.state();
    let task = state.current_task(agent).map(|t| t.task.id);
    let history = task
        .map(|t| history::history(store, agent, t))
        .unwrap_or_default();
    let diff = history
        .base
        .as_ref()
        .zip(git)
        .and_then(|(base, git)| branches::diff_since(git, agent, base));
    let mut pack = head(&state, agent, for_, git, &history);
    let running = state.agent(agent).map(|a| &a.processes);
    if let Some(running) = running.filter(|r| !r.is_empty()) {
        pack.push_str(
            "\nPROCESSES RUNNING IN YOUR SANDBOX (started with start_process, still serving; \
             stop one with stop_process):\n",
        );
        for (name, p) in running {
            let _ = writeln!(pack, "- {}: {}", name.0, p.command);
        }
    }
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
    let tail = transcript_tail(store, &state, agent, room);
    if !tail.is_empty() {
        let _ = write!(pack, "\nYOUR TRANSCRIPT, LATEST LAST:\n{tail}");
    }
    pack
}

/// The skills a pack carries in full, not just lists: how the CEO writes to the builder, which
/// applies to everything it sends and so cannot wait to be loaded.
fn always(position: guide::Position) -> &'static [&'static str] {
    match position {
        guide::Position::Ceo => &[x100_memory::EXECUTIVE_SUMMARY],
        guide::Position::Lead | guide::Position::Worker => &[],
    }
}

/// The skills a position is not told of: the CEO's way of writing to the builder is nothing a lead
/// or a worker uses.
fn hidden(position: guide::Position) -> &'static [&'static str] {
    match position {
        guide::Position::Ceo => &[],
        guide::Position::Lead | guide::Position::Worker => &[x100_memory::EXECUTIVE_SUMMARY],
    }
}

/// What `x100-memory` adds to an agent's pack.
#[derive(Default)]
struct Memory {
    /// The agent's index of notes, or nothing if it has none.
    index: String,
    skills: String,
}

/// Reads the agent's index and skills. A project without a code directory has no repositories, so
/// it carries the built-in skills alone. Memory that cannot be read leaves the pack without it
/// and says so, since a pack that cannot be built starts no driver.
fn memory(git: Option<&Git>, agent: AgentId, position: guide::Position) -> Memory {
    let always = always(position);
    let Some(git) = git else {
        return Memory {
            index: String::new(),
            skills: x100_memory::builtin_section(always),
        };
    };
    let mem = x100_memory::Memory::new(git.project_dir());
    let read = || -> x100_memory::Result<Memory> {
        Ok(Memory {
            index: mem.index(agent)?,
            skills: mem.skills_section(agent, always, hidden(position))?,
        })
    };
    read().unwrap_or_else(|e| {
        eprintln!("x100: could not read {agent}'s memory for its start pack: {e}");
        Memory {
            index: String::new(),
            skills: x100_memory::builtin_section(always),
        }
    })
}

/// The skills an agent's pack carries in full, and those it is not told of, by its position.
pub(crate) fn skill_rules(
    state: &ProjectState,
    agent: AgentId,
) -> (&'static [&'static str], &'static [&'static str]) {
    let position = match state.project().and_then(|p| p.team.position(agent)) {
        Some(Position::Ceo { .. }) => guide::Position::Ceo,
        Some(Position::Lead { .. }) => guide::Position::Lead,
        _ => guide::Position::Worker,
    };
    (always(position), hidden(position))
}

/// Everything but the calls, the diff and the transcript.
fn head(
    state: &ProjectState,
    agent: AgentId,
    for_: &Pack,
    git: Option<&Git>,
    history: &history::History,
) -> String {
    let Some(a) = state.agent(agent) else {
        return String::new();
    };
    let mut p = String::new();
    let _ = writeln!(p, "ROLE: {}", a.role);
    let _ = writeln!(p, "Your id: {agent}");
    if let Some(project) = state.project() {
        let _ = match &project.identity {
            Some(i) => write!(p, "\nPROJECT: {}\n{}\n", i.name, i.intent),
            None => write!(
                p,
                "\nPROJECT: not set up yet. It has no name, no intent and no code.\n"
            ),
        };
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
        let mem = memory(git, agent, position);
        if mem.index.trim().is_empty() {
            p.push_str("\nYOUR MEMORY: no notes yet.\n");
        } else {
            let _ = write!(p, "\nYOUR MEMORY:\n{}", mem.index);
        }
        if !mem.skills.is_empty() {
            let _ = write!(p, "\n{}", mem.skills);
        }
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
    if state.project().is_some_and(|p| p.ceo == agent) {
        let chat = chat(state);
        if !chat.is_empty() {
            let _ = write!(p, "\nYOUR CHAT WITH THE BUILDER, LATEST LAST:\n{chat}");
        }
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
        why::reason(state, agent, &for_.why, history)
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
    done.sort_by_key(|(t, _)| t.seq);
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

/// The last of the messages between the builder and the CEO, so a new driver of the CEO knows the
/// conversation, its own earlier answers included, even when its transcript has been cut.
fn chat(state: &ProjectState) -> String {
    let lines: Vec<String> = state
        .messages()
        .filter_map(|m| {
            let who = match m.message.route {
                Route::BuilderToCeo => "The builder",
                Route::CeoToBuilder => "You",
                _ => return None,
            };
            let mut body: String = m.message.body.chars().take(CHAT_CHARS).collect();
            if body.len() < m.message.body.len() {
                body.push('…');
            }
            let files = match m.message.attachments.len() {
                0 => String::new(),
                n => format!(" [{n} attached]"),
            };
            Some(format!("- {who}: {}{files}\n", body.replace('\n', "\n  ")))
        })
        .collect();
    let skip = lines.len().saturating_sub(CHAT_MESSAGES);
    lines[skip..].concat()
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

/// Waits, jobs it scheduled, tasks it delegated and has not accepted, messages awaiting a reply,
/// and its actions held for the builder.
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
    for h in state.held_actions().filter(|h| h.action.agent == agent) {
        let why = h.classification.as_ref().map_or("", |c| c.why.as_str());
        let _ = writeln!(
            b,
            "- Action {} is held for the builder's decision: {why}",
            h.action.id
        );
    }
    b
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

/// As much of the transcript's end as fits in `room` characters, oldest first. Lab drivers' runs
/// are recorded in the agent's transcript for the builder to see, and left out here: they were
/// trials, not the agent's work.
fn transcript_tail(
    store: &ProjectStore,
    state: &ProjectState,
    agent: AgentId,
    room: usize,
) -> String {
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
    let lab = |d: DriverId| {
        state
            .driver(d)
            .is_some_and(|r| r.driver.purpose != x100_core::driver::Purpose::Work)
    };
    for entry in entries.iter().rev() {
        if lab(entry.1.driver) {
            continue;
        }
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
