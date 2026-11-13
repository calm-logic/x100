//! Skills in the Agent Skills layout, `<name>/SKILL.md`, so a provider's agent can load one
//! unchanged. Versions are git history plus a `version` field that every committed change raises.

use x100_core::git::CommitId;
use x100_core::id::AgentId;
use x100_core::memory::{SkillName, SkillRef, SkillScope, SkillVersion};

use crate::error::{Error, Result};
use crate::format;
use crate::repo::Target;
use crate::{Memory, SKILL_LIMIT, SKILLS_LIMIT};

/// The skills x100 brings. They are written into a project's shared repository when it is made,
/// and a project's builder may edit them there like any other.
const BUILTIN: &[(&str, &str)] = &[(
    EXECUTIVE_SUMMARY,
    include_str!("../skills/executive-summary/SKILL.md"),
)];

/// The built-in skill for how the CEO writes to the builder. Only the CEO has it in its pack.
pub const EXECUTIVE_SUMMARY: &str = "executive-summary";

/// A skill as it stands on a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub skill: SkillRef,
    pub version: SkillVersion,
    pub description: String,
    /// The whole `SKILL.md`.
    pub text: String,
}

/// A built-in skill written into a repository that had none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeded {
    pub skill: SkillRef,
    pub version: SkillVersion,
    pub commit: CommitId,
}

fn path_of(name: &SkillName) -> String {
    format!("{}/SKILL.md", name.0)
}

fn read(skill: SkillRef, text: String) -> Skill {
    let meta = format::skill_meta(&text);
    Skill {
        skill,
        version: SkillVersion(meta.version),
        description: meta.description,
        text,
    }
}

impl Memory {
    /// Makes the shared repository with the built-in skills if it is not there. Each skill it
    /// writes is returned, for the caller to record.
    pub fn ensure_shared(&self) -> Result<Vec<Seeded>> {
        // Held to the end, so a second caller waits for the skills instead of finding a repository
        // that has none yet.
        static SEEDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = SEEDING.lock().unwrap_or_else(|e| e.into_inner());
        let repo = self.shared_repo();
        if !repo.ensure()? {
            return Ok(Vec::new());
        }
        let mut seeded = Vec::new();
        for (name, text) in BUILTIN {
            let skill = SkillRef {
                scope: SkillScope::Shared,
                name: SkillName((*name).to_owned()),
            };
            if let Some((version, commit)) =
                self.write_skill(&skill, text, &Target::Main, "x100: built-in skill")?
            {
                seeded.push(Seeded {
                    skill,
                    version,
                    commit,
                });
            }
        }
        Ok(seeded)
    }

    /// The skills of a scope on `target`, by name. A branch that is not there has none.
    pub fn skills(&self, scope: SkillScope, target: &Target) -> Result<Vec<Skill>> {
        let repo = self.skill_repo(scope);
        if !repo.exists() {
            return Ok(Vec::new());
        }
        let prefix = match scope {
            SkillScope::Shared => "",
            SkillScope::Agent(_) => "skills",
        };
        Ok(repo
            .files(target, prefix)?
            .into_iter()
            .filter_map(|(path, text)| {
                let rel = path.strip_prefix("skills/").unwrap_or(&path);
                let name = rel.strip_suffix("/SKILL.md").filter(|n| !n.contains('/'))?;
                let skill = SkillRef {
                    scope,
                    name: SkillName(name.to_owned()),
                };
                Some(read(skill, text))
            })
            .collect())
    }

    /// One skill on `target`.
    pub fn skill(&self, skill: &SkillRef, target: &Target) -> Result<Skill> {
        let repo = self.skill_repo(skill.scope);
        let missing = || Error::NotFound(format!("skill {}", skill.name.0));
        if !repo.exists() || !format::skill_name_ok(&skill.name.0) {
            return Err(missing());
        }
        let text = repo.file(target, &skill_path(skill))?.ok_or_else(missing)?;
        Ok(read(skill.clone(), text))
    }

    /// Writes a skill to `target` as one commit: its `version` is set to one past what main holds,
    /// or 1 for a new skill, so a change on a branch already carries the version it will have when
    /// merged. Returns the version and the commit, or `None` if the text changes nothing.
    pub fn write_skill(
        &self,
        skill: &SkillRef,
        text: &str,
        target: &Target,
        message: &str,
    ) -> Result<Option<(SkillVersion, CommitId)>> {
        if !format::skill_name_ok(&skill.name.0) {
            return Err(Error::Invalid(format!(
                "`{}` is not a skill name: lower-case words joined by single hyphens",
                skill.name.0
            )));
        }
        if text.len() > SKILL_LIMIT {
            return Err(Error::Invalid(format!(
                "the skill is {} bytes, over the {SKILL_LIMIT} a skill may hold",
                text.len()
            )));
        }
        let meta = format::skill_meta(text);
        if meta.name != skill.name.0 {
            return Err(Error::Invalid(format!(
                "the front matter's name is `{}`, not `{}`",
                meta.name, skill.name.0
            )));
        }
        if meta.description.is_empty() {
            return Err(Error::Invalid(
                "a skill needs a description in its front matter, which says when to use it".into(),
            ));
        }
        let repo = self.skill_repo(skill.scope);
        repo.ensure()?;
        let path = skill_path(skill);
        let on_main = repo.file(&Target::Main, &path)?;
        let version = on_main
            .as_deref()
            .map_or(1, |t| format::skill_meta(t).version + 1);
        let text = format::with_version(text, version);
        // The same text as main's, with only the version moved, is no change.
        let unchanged = |a: &str, b: &str| format::with_version(a, 0) == format::with_version(b, 0);
        if let (Target::Main, Some(old)) = (target, &on_main)
            && unchanged(old, &text)
        {
            return Ok(None);
        }
        Ok(repo
            .commit(target, &[(path, Some(text))], message)?
            .map(|commit| (SkillVersion(version), commit)))
    }

    /// What a start pack says of the skills `agent` may load: the shared ones and its own, each
    /// listed with how to load it, and the ones in `always` in full. The ones in `hidden` are
    /// left out, such as a skill that is for another role. Cut to [`SKILLS_LIMIT`]
    /// bytes, with a line counting what is left out.
    pub fn skills_section(
        &self,
        agent: AgentId,
        always: &[&str],
        hidden: &[&str],
    ) -> Result<String> {
        self.skills_section_on(agent, always, hidden, &Target::Main)
    }

    /// The same as [`Memory::skills_section`] with a trial branch in place of main, for a lab
    /// driver that tries a change. A repository that has no such branch is read at main.
    pub fn skills_section_on(
        &self,
        agent: AgentId,
        always: &[&str],
        hidden: &[&str],
        target: &Target,
    ) -> Result<String> {
        let at = |scope| -> Result<Target> {
            let repo = self.skill_repo(scope);
            Ok(match target {
                Target::Branch(_) if repo.exists() && repo.tip(target)?.is_none() => Target::Main,
                other => other.clone(),
            })
        };
        let (shared, own) = (SkillScope::Shared, SkillScope::Agent(agent));
        let mut all = self.skills(shared, &at(shared)?)?;
        all.extend(self.skills(own, &at(own)?)?);
        all.retain(|s| !hidden.contains(&s.skill.name.0.as_str()));
        Ok(section(&all, always))
    }
}

fn skill_path(skill: &SkillRef) -> String {
    match skill.scope {
        SkillScope::Shared => path_of(&skill.name),
        SkillScope::Agent(_) => format!("skills/{}", path_of(&skill.name)),
    }
}

/// The skills section for a project that has no code directory yet, so no repository: the
/// built-in skills alone.
pub fn builtin_section(always: &[&str]) -> String {
    let all: Vec<Skill> = BUILTIN
        .iter()
        .map(|(name, text)| {
            read(
                SkillRef {
                    scope: SkillScope::Shared,
                    name: SkillName((*name).to_owned()),
                },
                (*text).to_owned(),
            )
        })
        .collect();
    section(&all, always)
}

fn section(all: &[Skill], always: &[&str]) -> String {
    let mut out = String::new();
    // The skills to follow come first, so a long list never pushes them out.
    let (full, listed): (Vec<&Skill>, Vec<&Skill>) = all
        .iter()
        .partition(|s| always.contains(&s.skill.name.0.as_str()));
    for s in &full {
        let part = format!(
            "SKILL {} (follow it where it says it applies):\n{}\n",
            s.skill.name.0, s.text
        );
        if out.len() + part.len() > SKILLS_LIMIT {
            break;
        }
        out.push_str(&part);
    }
    if listed.is_empty() {
        return out;
    }
    out.push_str("SKILLS YOU CAN LOAD (`load_skill`, with the scope shown and the name):\n");
    // Room for the line that counts what is left out.
    let reserve = 80;
    let mut shown = 0;
    for s in &listed {
        let scope = match s.skill.scope {
            SkillScope::Shared => "shared".to_owned(),
            SkillScope::Agent(a) => format!("your own, {a}"),
        };
        let line = format!(
            "- {} (version {}, {scope}): {}\n",
            s.skill.name.0, s.version.0, s.description
        );
        if out.len() + line.len() + reserve > SKILLS_LIMIT {
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    if shown < listed.len() {
        out.push_str(&format!(
            "- ... and {} more skills, not listed here\n",
            listed.len() - shown
        ));
    }
    out
}
