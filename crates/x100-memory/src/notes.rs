//! An agent's notes: one Markdown file each in its repository's `memory/`, with the events they
//! came from in the front matter, and the index kept from them.

use x100_core::event::Seq;
use x100_core::git::CommitId;
use x100_core::id::AgentId;
use x100_core::memory::NoteName;
use x100_core::time::Timestamp;

use crate::error::{Error, Result};
use crate::format::{self, Entry};
use crate::repo::{Change, Target};
use crate::{INDEX, INDEX_LIMIT, Memory, NOTE_LIMIT};

const DIR: &str = "memory";
const SUMMARY_CHARS: usize = 100;

/// One note, as it stands on main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub name: NoteName,
    pub written: Timestamp,
    /// The events the note was drawn from, so reflection can check it against the log.
    pub sources: Vec<Seq>,
    pub body: String,
    /// The commit main was at when it was read.
    pub commit: CommitId,
}

/// A note that matched a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteHit {
    pub name: NoteName,
    pub summary: String,
}

/// A change to the notes of one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteChange {
    Write {
        name: NoteName,
        body: String,
        sources: Vec<Seq>,
    },
    Delete(NoteName),
}

impl Memory {
    /// Writes a note, replacing one of the same name, and brings the index with it, in one commit
    /// on main. Returns the commit and the name it was kept under.
    pub fn remember(
        &self,
        agent: AgentId,
        name: &NoteName,
        body: &str,
        sources: &[Seq],
        at: Timestamp,
    ) -> Result<(NoteName, CommitId)> {
        let name = format::note_name(name)?;
        let change = NoteChange::Write {
            name: name.clone(),
            body: body.to_owned(),
            sources: sources.to_vec(),
        };
        let message = format!("remember {}", name.0);
        let commit = self
            .change_notes(agent, &Target::Main, &[change], at, &message)?
            .ok_or_else(|| Error::Invalid(format!("{} already says that", name.0)))?;
        Ok((name, commit))
    }

    /// Applies `changes` to an agent's notes on `target` as one commit, with the index remade from
    /// the result. `None` if nothing changed. Reflection compacts memory with this, on main or on a
    /// trial branch.
    pub fn change_notes(
        &self,
        agent: AgentId,
        target: &Target,
        changes: &[NoteChange],
        at: Timestamp,
        message: &str,
    ) -> Result<Option<CommitId>> {
        let repo = self.agent_repo(agent);
        repo.ensure()?;
        // A branch not yet made starts from main.
        let from = match repo.tip(target)? {
            Some(_) => target.clone(),
            None => Target::Main,
        };
        let mut notes: std::collections::BTreeMap<String, String> = repo
            .files(&from, DIR)?
            .into_iter()
            .filter(|(p, _)| p.ends_with(".md") && p != &format!("{DIR}/{INDEX}"))
            .collect();
        let mut files: Vec<Change> = Vec::new();
        for change in changes {
            match change {
                NoteChange::Write {
                    name,
                    body,
                    sources,
                } => {
                    let name = format::note_name(name)?;
                    if body.trim().is_empty() {
                        return Err(Error::Invalid("a note with nothing in it".into()));
                    }
                    if body.len() > NOTE_LIMIT {
                        return Err(Error::Invalid(format!(
                            "the note is {} bytes, over the {NOTE_LIMIT} a note may hold: \
                             split it into notes that each say one thing",
                            body.len()
                        )));
                    }
                    let path = format!("{DIR}/{}", name.0);
                    let text = format::encode_note(at, sources, body);
                    notes.insert(path.clone(), text.clone());
                    files.push((path, Some(text)));
                }
                NoteChange::Delete(name) => {
                    let path = format!("{DIR}/{}", format::note_name(name)?.0);
                    notes.remove(&path);
                    files.push((path, None));
                }
            }
        }
        let entries = notes
            .iter()
            .map(|(path, text)| {
                let stored = format::decode_note(text);
                Entry {
                    name: path.trim_start_matches(&format!("{DIR}/")).to_owned(),
                    written: stored.written,
                    summary: format::summary(&stored.body, SUMMARY_CHARS),
                }
            })
            .collect();
        files.push((
            format!("{DIR}/{INDEX}"),
            Some(format::render_index(entries, INDEX_LIMIT)),
        ));
        repo.commit(target, &files, message)
    }

    /// The agent's index as it stands on main, or nothing if it has no notes.
    pub fn index(&self, agent: AgentId) -> Result<String> {
        self.index_on(agent, &Target::Main)
    }

    /// The agent's index on a trial branch, or at main if the agent has no such branch.
    pub fn index_on(&self, agent: AgentId, target: &Target) -> Result<String> {
        let repo = self.agent_repo(agent);
        if !repo.exists() {
            return Ok(String::new());
        }
        let target = match repo.tip(target)? {
            Some(_) => target,
            None => &Target::Main,
        };
        Ok(repo
            .file(target, &format!("{DIR}/{INDEX}"))?
            .unwrap_or_default())
    }

    /// One note as it stands on main, and the commit main was at.
    pub fn note(&self, agent: AgentId, name: &NoteName) -> Result<Note> {
        let name = format::note_name(name)?;
        let repo = self.agent_repo(agent);
        let missing = || Error::NotFound(format!("note {}", name.0));
        if !repo.exists() {
            return Err(missing());
        }
        let text = repo
            .file(&Target::Main, &format!("{DIR}/{}", name.0))?
            .ok_or_else(missing)?;
        let stored = format::decode_note(&text);
        Ok(Note {
            name,
            written: stored.written,
            sources: stored.sources,
            body: stored.body,
            commit: repo.main_tip()?,
        })
    }

    /// Every note on main, newest first, with the commit main was at.
    pub fn notes(&self, agent: AgentId) -> Result<Vec<Note>> {
        let repo = self.agent_repo(agent);
        if !repo.exists() {
            return Ok(Vec::new());
        }
        let commit = repo.main_tip()?;
        let mut notes: Vec<Note> = repo
            .files(&Target::Main, DIR)?
            .into_iter()
            .filter(|(p, _)| p.ends_with(".md") && p != &format!("{DIR}/{INDEX}"))
            .map(|(path, text)| {
                let stored = format::decode_note(&text);
                Note {
                    name: NoteName(path.trim_start_matches(&format!("{DIR}/")).to_owned()),
                    written: stored.written,
                    sources: stored.sources,
                    body: stored.body,
                    commit: commit.clone(),
                }
            })
            .collect();
        notes.sort_by(|a, b| b.written.cmp(&a.written).then_with(|| a.name.cmp(&b.name)));
        Ok(notes)
    }

    /// The notes on main that say all of `query`'s words, best first, at most `limit`. A word in
    /// a note's name counts the same as one in its text.
    pub fn search_notes(&self, agent: AgentId, query: &str, limit: usize) -> Result<Vec<NoteHit>> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let mut scored: Vec<(usize, Note)> = self
            .notes(agent)?
            .into_iter()
            .filter_map(|n| {
                let text = format!("{} {}", n.name.0, n.body).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str())).then(|| {
                    (
                        words.iter().map(|w| text.matches(w.as_str()).count()).sum(),
                        n,
                    )
                })
            })
            .collect();
        // Stable, so equal scores stay newest first.
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(_, n)| NoteHit {
                summary: format::summary(&n.body, SUMMARY_CHARS),
                name: n.name,
            })
            .collect())
    }
}
