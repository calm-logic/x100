//! Doing what a memory call planned: reading and writing notes and skills through `x100-memory`,
//! and searching the log's text. It runs under the project's writer lock, on a blocking thread,
//! before the call's events are appended, and returns the events it caused and what the driver
//! reads.
//!
//! Every file goes through `x100-memory`. The log is searched here because it is x100-store's,
//! and a search is a read, so it changes nothing.

use std::path::Path;

use serde_json::json;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::id::AgentId;
use x100_core::message::Route;
use x100_core::party::Party;
use x100_core::task::TaskEvent;
use x100_core::time::Timestamp;
use x100_memory::{Memory, Target};
use x100_store::ProjectStore;

use crate::tools::{MemoryPlan, who};

/// What a memory call did: events to append, and the reply.
pub(crate) struct Done {
    pub events: Vec<ProjectEvent>,
    pub reply: String,
}

/// Makes the project's shared skills repository, with the built-in skills, if it has a code
/// directory and no such repository yet, and records each skill written as a committed change. A
/// repository already there costs one look. Blocks; takes the project's writer lock only to
/// append.
pub(crate) fn ensure_shared(ctx: &crate::ctx::ProjectCtx) {
    let Some(dir) = ctx.git().map(|g| g.project_dir().to_owned()) else {
        return;
    };
    match Memory::new(&dir).ensure_shared() {
        Ok(seeded) if !seeded.is_empty() => {
            let events = seeded
                .into_iter()
                .map(|s| {
                    (
                        x100_core::party::Actor::Daemon,
                        ProjectEvent::SkillCommitted {
                            skill: s.skill,
                            version: s.version,
                            commit: s.commit,
                            trial: None,
                        },
                    )
                })
                .collect();
            if let Err(e) = ctx.writer().append(events) {
                eprintln!("x100: could not record the built-in skills: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("x100: could not make the shared skills repository: {e}"),
    }
}

/// An event a search found, the kind it is called, and what it says.
type Hit = (Envelope<ProjectEvent>, &'static str, String);

/// How much of one event a search result shows, in characters.
const SNIPPET: usize = 400;
const PAGE: usize = 200;

pub(crate) fn run(
    store: &ProjectStore,
    code_dir: &Path,
    agent: AgentId,
    plan: &MemoryPlan,
    now: Timestamp,
) -> Result<Done, String> {
    let memory = Memory::new(code_dir);
    let fail = |e: x100_memory::Error| e.to_string();
    match plan {
        MemoryPlan::Remember {
            note,
            body,
            sources,
        } => {
            let (name, commit) = memory
                .remember(agent, note, body, sources, now)
                .map_err(fail)?;
            let reply = json!({ "note": name, "commit": commit.0 }).to_string();
            Ok(Done {
                events: vec![ProjectEvent::MemoryWritten {
                    agent,
                    note: name,
                    commit,
                }],
                reply,
            })
        }
        MemoryPlan::ReadNote { note } => {
            let read = memory.note(agent, note).map_err(fail)?;
            let cites = if read.sources.is_empty() {
                String::new()
            } else {
                let list: Vec<String> = read.sources.iter().map(|s| s.0.to_string()).collect();
                format!(", drawn from log events {}", list.join(", "))
            };
            let reply = format!(
                "Note {} (written at {}{cites}):\n\n{}",
                read.name.0,
                read.written.as_millis(),
                read.body
            );
            Ok(Done {
                events: vec![ProjectEvent::NoteRead {
                    agent,
                    note: read.name,
                    commit: read.commit,
                }],
                reply,
            })
        }
        MemoryPlan::SearchNotes { query } => {
            let hits = memory.search_notes(agent, query, 20).map_err(fail)?;
            let reply = if hits.is_empty() {
                "No note of yours says all of those words.".to_owned()
            } else {
                hits.iter()
                    .map(|h| format!("- {}: {}", h.name.0, h.summary))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(Done {
                events: Vec::new(),
                reply,
            })
        }
        MemoryPlan::LoadSkill { skill } => {
            let loaded = memory.skill(skill, &Target::Main).map_err(fail)?;
            Ok(Done {
                events: vec![ProjectEvent::SkillLoaded {
                    agent,
                    skill: loaded.skill,
                    version: loaded.version,
                }],
                reply: loaded.text,
            })
        }
        MemoryPlan::ReadMemories { agent: whose } => {
            let notes = memory.notes(*whose).map_err(fail)?;
            let reply = if notes.is_empty() {
                format!("{whose} has no notes.")
            } else {
                notes
                    .iter()
                    .map(|n| {
                        let list: Vec<String> = n.sources.iter().map(|s| s.0.to_string()).collect();
                        format!(
                            "## {} (written at {}, cites events [{}])\n\n{}",
                            n.name.0,
                            n.written.as_millis(),
                            list.join(", "),
                            n.body
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            // A reflection's reading is not the agent using its notes.
            Ok(Done {
                events: Vec::new(),
                reply,
            })
        }
        MemoryPlan::ReadLog { from, to } => crate::reflect::read_log(store, *from, *to),
        MemoryPlan::Correct {
            agent: whose,
            note,
            body,
            sources,
        } => crate::reflect::correct(&memory, *whose, note, body, sources, now),
        MemoryPlan::Commit { change } => crate::reflect::commit(&memory, change, now),
        MemoryPlan::SearchLog { query, limit } => {
            let hits = search_log(store, query, *limit)?;
            let reply = if hits.is_empty() {
                "Nothing in the log has all of those words.".to_owned()
            } else {
                let terms = terms(query);
                store.log.with_state(|state| {
                    hits.iter()
                        .map(|(e, kind, text)| {
                            format!(
                                "#{} at {} {} {}: {}",
                                e.seq.0,
                                e.at.as_millis(),
                                kind,
                                by(state, e),
                                snippet(text, &terms)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            };
            Ok(Done {
                events: Vec::new(),
                reply,
            })
        }
    }
}

fn terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .take(crate::tools::QUERY_WORDS)
        .map(str::to_lowercase)
        .collect()
}

/// The newest events whose words include every one of the query's, among the kinds that carry
/// what people and agents wrote. The calls an agent made are left out: what they did is in the
/// events they caused, and a search would otherwise find itself.
fn search_log(store: &ProjectStore, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
    let terms = terms(query);
    let words: Vec<&str> = terms.iter().map(String::as_str).collect();
    let mut found = Vec::new();
    let mut before: Option<Seq> = None;
    loop {
        let page = store
            .log
            .matching(&words, before, PAGE)
            .map_err(|e| format!("the log could not be searched: {e}"))?;
        let Some(last) = page.last().map(|e| e.seq) else {
            break;
        };
        let more = page.len() == PAGE;
        before = Some(last);
        for e in page {
            let Some((kind, text)) = readable(&e.event) else {
                continue;
            };
            // The stored text also holds keys and ids, so the words are checked in the prose.
            let lower = text.to_lowercase();
            if terms.iter().all(|t| lower.contains(t.as_str())) {
                found.push((e, kind, text));
                if found.len() == limit {
                    return Ok(found);
                }
            }
        }
        if !more {
            break;
        }
    }
    Ok(found)
}

/// What an event says in words, for the kinds a search covers.
pub(crate) fn readable(event: &ProjectEvent) -> Option<(&'static str, String)> {
    Some(match event {
        ProjectEvent::MessageSent(m) => ("message", m.body.clone()),
        ProjectEvent::TaskOpened { task, .. } => ("task", format!("{}: {}", task.title, task.ask)),
        ProjectEvent::Task { task, event } => match event {
            TaskEvent::Reported(r) => ("report", format!("on {task}: {}", r.summary)),
            TaskEvent::ProgressNoted(p) => ("progress", format!("on {task}: {}", p.note)),
            TaskEvent::Reviewed(x100_core::task::Verdict::SendBack { note, .. }) => {
                ("sent back", format!("{task}: {note}"))
            }
            TaskEvent::MergeFailed { conflict } => ("merge failed", format!("{task}: {conflict}")),
            _ => return None,
        },
        ProjectEvent::SpecWritten(s) => ("spec", s.body.clone()),
        ProjectEvent::SetUp(i) | ProjectEvent::Renamed(i) => {
            ("project", format!("{}: {}", i.name, i.intent))
        }
        _ => return None,
    })
}

fn by(state: &x100_core::fold::ProjectState, e: &Envelope<ProjectEvent>) -> String {
    match &e.event {
        ProjectEvent::MessageSent(m) => match m.route {
            Route::BuilderToCeo => "from the builder to the CEO".into(),
            Route::CeoToBuilder => "from the CEO to the builder".into(),
            Route::DaemonToBuilder => "from the daemon to the builder".into(),
            Route::Between { from, to } => format!(
                "from {} to {}",
                who(state, Party::Agent(from)),
                who(state, Party::Agent(to))
            ),
        },
        _ => match e.by {
            x100_core::party::Actor::Builder => "by the builder".into(),
            x100_core::party::Actor::Agent(a) => format!("by {}", who(state, Party::Agent(a))),
            x100_core::party::Actor::Daemon => "by the daemon".into(),
        },
    }
}

/// About `SNIPPET` characters of `text` around the first term in it.
fn snippet(text: &str, terms: &[String]) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= SNIPPET {
        return flat;
    }
    let lower = flat.to_lowercase();
    let at = terms
        .iter()
        .filter_map(|t| lower.find(t.as_str()))
        .min()
        .map_or(0, |byte| lower[..byte].chars().count());
    let start = at.saturating_sub(SNIPPET / 4);
    let end = (start + SNIPPET).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out.insert_str(0, "...");
    }
    if end < chars.len() {
        out.push_str("...");
    }
    out
}
