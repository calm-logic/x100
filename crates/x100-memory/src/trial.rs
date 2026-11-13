//! What the lab needs of the repositories: reading a change on trial where it is and main
//! elsewhere, measuring an agent's memory before and after a compaction, turning a compaction into
//! the note changes it makes, and merging a compaction into main note by note.

use std::collections::{BTreeMap, BTreeSet};

use x100_core::event::Seq;
use x100_core::git::CommitId;
use x100_core::id::AgentId;
use x100_core::memory::{NoteName, SkillRef};
use x100_core::time::Timestamp;

use crate::error::{Error, Result};
use crate::format;
use crate::notes::{DIR, NoteChange};
use crate::repo::Target;
use crate::skills::Skill;
use crate::{INDEX, Memory};

/// How much an agent's memory holds on a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// The notes' files and the index, in bytes, as stored.
    pub bytes: usize,
    pub notes: usize,
    /// Notes the index has no room to list: a full index is over its limit.
    pub unlisted: usize,
}

impl Size {
    /// Whether the index lists every note within [`crate::INDEX_LIMIT`].
    pub fn index_fits(&self) -> bool {
        self.unlisted == 0
    }
}

impl Memory {
    /// A skill on `target`, or on main where its repository has no such branch.
    pub fn skill_on(&self, skill: &SkillRef, target: &Target) -> Result<Skill> {
        let repo = self.skill_repo(skill.scope);
        if !repo.exists() {
            return Err(Error::NotFound(format!("skill {}", skill.name.0)));
        }
        self.skill(skill, &repo.or_main(target)?)
    }

    /// The size of an agent's memory on `target`, or on main if it has no such branch.
    pub fn size_on(&self, agent: AgentId, target: &Target) -> Result<Size> {
        let repo = self.agent_repo(agent);
        if !repo.exists() {
            return Ok(Size {
                bytes: 0,
                notes: 0,
                unlisted: 0,
            });
        }
        let files = repo.files(&repo.or_main(target)?, DIR)?;
        let index_path = format!("{DIR}/{INDEX}");
        let notes = files
            .keys()
            .filter(|p| p.ends_with(".md") && **p != index_path)
            .count();
        let listed = files
            .get(&index_path)
            .map_or(0, |i| i.lines().filter(|l| l.starts_with("- [")).count());
        Ok(Size {
            bytes: files.values().map(String::len).sum(),
            notes,
            unlisted: notes.saturating_sub(listed),
        })
    }

    /// The note changes that leave an agent's memory holding exactly `notes`, compared with what
    /// main holds now. A note kept under its name keeps the events it cites; a note that is new
    /// or rewritten also cites every event the removed notes cited, since what they said went
    /// into it. Refused if it keeps no note or names one twice.
    pub fn compaction(
        &self,
        agent: AgentId,
        notes: &[(NoteName, String)],
    ) -> Result<Vec<NoteChange>> {
        if notes.is_empty() {
            return Err(Error::Invalid(
                "a compaction that keeps no note: name the notes the memory should hold".into(),
            ));
        }
        let mut named = BTreeSet::new();
        let mut wanted = Vec::new();
        for (name, body) in notes {
            let name = format::note_name(name)?;
            if !named.insert(name.clone()) {
                return Err(Error::Invalid(format!(
                    "the note {} is named twice",
                    name.0
                )));
            }
            wanted.push((name, body.trim().to_owned()));
        }
        let now: BTreeMap<NoteName, (Vec<Seq>, String)> = self
            .notes(agent)?
            .into_iter()
            .map(|n| (n.name, (n.sources, n.body.trim().to_owned())))
            .collect();
        let removed: BTreeSet<Seq> = now
            .iter()
            .filter(|(name, _)| !named.contains(*name))
            .flat_map(|(_, (sources, _))| sources.iter().copied())
            .collect();
        let mut changes: Vec<NoteChange> = now
            .keys()
            .filter(|name| !named.contains(*name))
            .map(|name| NoteChange::Delete(name.clone()))
            .collect();
        for (name, body) in wanted {
            let kept = now.get(&name);
            if kept.is_some_and(|(_, old)| *old == body) {
                continue;
            }
            let mut sources: BTreeSet<Seq> = kept
                .map(|(s, _)| s.iter().copied().collect())
                .unwrap_or_default();
            sources.extend(removed.iter().copied());
            changes.push(NoteChange::Write {
                name,
                body,
                sources: sources.into_iter().collect(),
            });
        }
        Ok(changes)
    }

    /// Merges a compaction on trial into main note by note, and deletes the branch. Notes the
    /// agent wrote on main meanwhile stay, and the index is remade from the result, so the index
    /// itself never conflicts. A note changed both on main and on the branch since the branch
    /// began is a conflict: nothing is changed and the branch is kept.
    pub fn merge_notes(&self, agent: AgentId, branch: &str, at: Timestamp) -> Result<CommitId> {
        let repo = self.agent_repo(agent);
        let target = Target::Branch(branch.to_owned());
        if !repo.exists() || repo.tip(&target)?.is_none() {
            return Err(Error::NotFound(format!("branch {branch}")));
        }
        let base = repo.base(branch)?;
        let notes = |files: BTreeMap<String, String>| -> BTreeMap<String, String> {
            files
                .into_iter()
                .filter(|(p, _)| p.ends_with(".md") && *p != format!("{DIR}/{INDEX}"))
                .collect()
        };
        let before = notes(repo.files_at(&base, DIR)?);
        let after = notes(repo.files(&target, DIR)?);
        let main = notes(repo.files(&Target::Main, DIR)?);
        let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        let mut changes = Vec::new();
        for path in paths {
            let (was, is) = (before.get(path), after.get(path));
            if was == is {
                continue;
            }
            if main.get(path) != was {
                return Err(Error::Conflict {
                    branch: branch.to_owned(),
                    why: format!("{path} changed on main while the compaction was on trial"),
                });
            }
            let name = NoteName(path.trim_start_matches(&format!("{DIR}/")).to_owned());
            changes.push(match is {
                Some(text) => {
                    let stored = format::decode_note(text);
                    NoteChange::Write {
                        name,
                        body: stored.body,
                        sources: stored.sources,
                    }
                }
                None => NoteChange::Delete(name),
            });
        }
        let message = format!("x100: merge {branch}");
        let commit = match self.change_notes(agent, &Target::Main, &changes, at, &message)? {
            Some(c) => c,
            None => repo.main_tip()?,
        };
        repo.discard(branch)?;
        Ok(commit)
    }
}
