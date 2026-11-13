//! The memory and log tools: `remember`, `read_note`, `search_notes`, `load_skill`,
//! `search_log` and the CEO's `read_memories`. What they do is read or write files and the log's
//! text, which a pure planner cannot, so the planner only checks what the folded state decides and
//! names the work in a [`MemoryPlan`]. The desk runs it ([`crate::recall`]) before it appends, since
//! the commit it makes goes into the event.

use x100_core::event::Seq;
use x100_core::id::AgentId;
use x100_core::memory::{NoteName, SkillRef, SkillScope};
use x100_memory::Memory;

use super::{At, Plan, Refusal};

/// How many matches `search_log` returns when it is not told, and the most it ever does.
pub(crate) const SEARCH_DEFAULT: usize = 10;
pub(crate) const SEARCH_MAX: usize = 50;
/// Words in a query that count, so a pasted paragraph is not a query of a hundred terms.
pub(crate) const QUERY_WORDS: usize = 8;

/// What the desk does for a memory call that passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemoryPlan {
    Remember {
        note: NoteName,
        body: String,
        sources: Vec<Seq>,
    },
    ReadNote {
        note: NoteName,
    },
    SearchNotes {
        query: String,
    },
    LoadSkill {
        skill: SkillRef,
    },
    ReadMemories {
        agent: AgentId,
    },
    SearchLog {
        query: String,
        limit: usize,
    },
}

fn plan(memory: MemoryPlan) -> Plan {
    Plan {
        memory: Some(memory),
        ..Plan::default()
    }
}

/// Notes and skills are files in the project's code directory, which a project gets when it is
/// set up.
fn need_code_dir(at: &At) -> Result<(), Refusal> {
    match at.project.code_dir {
        Some(_) => Ok(()),
        None => Err(
            "memory starts when the project is set up: it is kept in the project's \
                     code directory, which the project does not have yet"
                .into(),
        ),
    }
}

fn query(text: &str) -> Result<(), Refusal> {
    if text.split_whitespace().next().is_none() {
        return Err("the query has no words".into());
    }
    Ok(())
}

pub(crate) fn remember(
    at: &At,
    note: &NoteName,
    body: &str,
    sources: &[Seq],
) -> Result<Plan, Refusal> {
    need_code_dir(at)?;
    let note = Memory::note_name(note).map_err(|e| e.to_string())?;
    if body.trim().is_empty() {
        return Err("the note is empty".into());
    }
    let last = at.state.seq().map_or(0, |s| s.0);
    if let Some(bad) = sources.iter().find(|s| s.0 == 0 || s.0 > last) {
        return Err(format!(
            "event {} is not in the log, so a note cannot cite it: use the numbers `search_log` shows",
            bad.0
        ));
    }
    Ok(plan(MemoryPlan::Remember {
        note,
        body: body.to_owned(),
        sources: sources.to_vec(),
    }))
}

pub(crate) fn read_note(at: &At, note: &NoteName) -> Result<Plan, Refusal> {
    need_code_dir(at)?;
    let note = Memory::note_name(note).map_err(|e| e.to_string())?;
    Ok(plan(MemoryPlan::ReadNote { note }))
}

pub(crate) fn search_notes(at: &At, text: &str) -> Result<Plan, Refusal> {
    need_code_dir(at)?;
    query(text)?;
    Ok(plan(MemoryPlan::SearchNotes {
        query: text.to_owned(),
    }))
}

pub(crate) fn load_skill(at: &At, skill: &SkillRef) -> Result<Plan, Refusal> {
    need_code_dir(at)?;
    match skill.scope {
        SkillScope::Agent(owner) if owner != at.id() => {
            return Err(format!(
                "the skill belongs to {owner}: you load shared skills and your own"
            ));
        }
        _ => {}
    }
    Ok(plan(MemoryPlan::LoadSkill {
        skill: skill.clone(),
    }))
}

pub(crate) fn read_memories(at: &At, agent: AgentId) -> Result<Plan, Refusal> {
    need_code_dir(at)?;
    at.live_agent(agent)?;
    Ok(plan(MemoryPlan::ReadMemories { agent }))
}

pub(crate) fn search_log(_at: &At, text: &str, limit: Option<u32>) -> Result<Plan, Refusal> {
    query(text)?;
    let limit = limit
        .map_or(SEARCH_DEFAULT, |l| l as usize)
        .clamp(1, SEARCH_MAX);
    Ok(plan(MemoryPlan::SearchLog {
        query: text.to_owned(),
        limit,
    }))
}
