//! The memory and skill repositories: what agents keep between drivers and tasks, as Markdown in
//! folders under git that the builder can read.
//!
//! ```text
//! <code dir>/.x100/skills/                  one repository, shared in the project
//!     <name>/SKILL.md
//! <code dir>/.x100/agents/<agent id>/       one repository per agent
//!     memory/INDEX.md                       the index, kept from the notes
//!     memory/<note>.md                      one note per file, with front matter
//!     skills/<name>/SKILL.md                the agent's own skills
//! ```
//!
//! Each repository has a `main` branch, which agents load from and which is checked out, and
//! `trial/...` branches for changes that are on trial. Every change is a commit, and the commit is
//! returned so the caller can put it in an event. Nothing but this crate reads or writes these
//! files. Its interface is what a later storage engine with several writers will stand behind, so
//! it speaks in notes, skills and commits and not in paths.
//!
//! The index is what a start pack carries. It is cut to [`INDEX_LIMIT`] bytes however many notes
//! there are, and the skills in a pack to [`SKILLS_LIMIT`], so what memory adds to a pack has a
//! ceiling.

mod error;
mod format;
mod git;
mod notes;
mod repo;
mod skills;

use std::path::{Path, PathBuf};

use x100_core::id::AgentId;
use x100_core::memory::{NoteName, SkillScope};

pub use error::{Error, Result};
pub use notes::{Note, NoteChange, NoteHit};
pub use repo::Target;
pub use skills::{EXECUTIVE_SUMMARY, Seeded, Skill, builtin_section};

use repo::Repo;

/// The most bytes of index a start pack carries.
pub const INDEX_LIMIT: usize = 4 * 1024;
/// The most bytes of skills, listed and in full, a start pack carries.
pub const SKILLS_LIMIT: usize = 6 * 1024;
/// The longest note, in bytes. A longer one is two notes.
pub const NOTE_LIMIT: usize = 32 * 1024;
/// The longest skill, in bytes.
pub const SKILL_LIMIT: usize = 64 * 1024;
const NAME_LIMIT: usize = 64;
const INDEX: &str = "INDEX.md";

/// The repositories of one project, under its code directory's `.x100/`.
#[derive(Debug, Clone)]
pub struct Memory {
    root: PathBuf,
}

impl Memory {
    /// The repositories under `code_dir`. Nothing is made until something is written.
    pub fn new(code_dir: &Path) -> Memory {
        Memory {
            root: code_dir.join(".x100"),
        }
    }

    fn agent_repo(&self, agent: AgentId) -> Repo {
        Repo::at(self.root.join("agents").join(agent.to_string()))
    }

    fn shared_repo(&self) -> Repo {
        Repo::at(self.root.join("skills"))
    }

    fn skill_repo(&self, scope: SkillScope) -> Repo {
        match scope {
            SkillScope::Shared => self.shared_repo(),
            SkillScope::Agent(a) => self.agent_repo(a),
        }
    }

    /// Whether `agent` has a repository yet.
    pub fn has_notes(&self, agent: AgentId) -> bool {
        self.agent_repo(agent).exists()
    }

    /// The name a note is kept under: `deploys` and `deploys.md` are the same note.
    pub fn note_name(name: &NoteName) -> Result<NoteName> {
        format::note_name(name)
    }

    /// Merges a trial branch of a skill or memory repository into main, and deletes the branch.
    /// `scope` names the repository: the shared skills, or an agent's.
    pub fn merge(&self, scope: SkillScope, branch: &str) -> Result<x100_core::git::CommitId> {
        self.skill_repo(scope).merge(branch)
    }

    /// Deletes a trial branch, which drops the change on it.
    pub fn discard(&self, scope: SkillScope, branch: &str) -> Result<()> {
        self.skill_repo(scope).discard(branch)
    }

    /// The trial branches a repository has.
    pub fn branches(&self, scope: SkillScope) -> Result<Vec<String>> {
        let repo = self.skill_repo(scope);
        if !repo.exists() {
            return Ok(Vec::new());
        }
        repo.branches()
    }
}
