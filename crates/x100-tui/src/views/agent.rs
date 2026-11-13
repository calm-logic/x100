//! The pages of an agent beside its output: its notes, its skills and the trials about them.

use x100_client::words;
use x100_core::id::AgentId;
use x100_core::memory::SkillScope;
use x100_proto::{Memories, Skills};

use super::Ctx;
use super::settings::{trial_change, trial_state};
use crate::page::{BOLD, DIM, Page};

pub fn memories(c: &Ctx, loaded: &Option<Result<Memories, String>>) -> Page {
    let mut page = Page::new();
    match loaded {
        None => {
            page.styled("Reading its notes…", DIM);
        }
        Some(Err(e)) => {
            page.text(format!("Its notes could not be read: {e}"));
        }
        Some(Ok(m)) if m.notes.is_empty() => {
            page.text("It has written no notes yet.");
        }
        Some(Ok(m)) => {
            for n in &m.notes {
                page.styled(
                    format!("{} · written {}", n.name.0, words::moment(n.written)),
                    BOLD,
                );
                for line in n.body.lines() {
                    page.paragraph(line, "  ", c.width);
                }
                page.blank();
            }
        }
    }
    page
}

pub fn skills(c: &Ctx, agent: AgentId, loaded: &Option<Result<Skills, String>>) -> Page {
    let mut page = Page::new();
    match loaded {
        None => {
            page.styled("Reading its skills…", DIM);
        }
        Some(Err(e)) => {
            page.text(format!("Its skills could not be read: {e}"));
        }
        Some(Ok(s)) if s.skills.is_empty() => {
            page.text("No skills yet.");
        }
        Some(Ok(s)) => {
            for scope in [SkillScope::Agent(agent), SkillScope::Shared] {
                let mine: Vec<_> = s.skills.iter().filter(|k| k.skill.scope == scope).collect();
                if mine.is_empty() {
                    continue;
                }
                page.heading(match scope {
                    SkillScope::Shared => "Shared skills",
                    SkillScope::Agent(_) => "Its own skills",
                });
                for k in mine {
                    page.styled(
                        format!("  {} · version {}", k.skill.name.0, k.version.0),
                        BOLD,
                    );
                    page.paragraph(&k.description, "    ", c.width);
                }
                page.blank();
            }
        }
    }
    page
}

pub fn trials(c: &Ctx, agent: AgentId) -> Page {
    let mut page = Page::new();
    let mut any = false;
    for t in c.p.trials_about(agent) {
        any = true;
        page.styled(
            format!(
                "{} · {}",
                trial_change(c, &t.trial.change),
                trial_state(t.state)
            ),
            BOLD,
        );
        if let Some(why) = &t.why {
            page.paragraph(why, "  ", c.width);
        }
    }
    if !any {
        page.text("No trial is about its skills or notes.");
    }
    page
}

#[cfg(test)]
mod tests {
    use x100_core::fold::{DaemonState, ProjectState};
    use x100_core::git::CommitId;
    use x100_core::memory::{NoteName, SkillName, SkillRef, SkillVersion};
    use x100_core::time::Timestamp;
    use x100_proto::{MemoryNote, SkillInfo};

    use super::*;

    fn ctx<'a>(p: &'a ProjectState, d: &'a DaemonState) -> Ctx<'a> {
        Ctx { p, d, width: 80 }
    }

    #[test]
    fn notes_and_skills_are_listed_with_what_the_daemon_read() {
        let (p, d) = (ProjectState::default(), DaemonState::default());
        let agent = AgentId::from_u128(7);
        let notes = Memories {
            notes: vec![MemoryNote {
                name: NoteName("Deploys".into()),
                commit: CommitId("0123456789abcdef0123456789abcdef01234567".into()),
                written: Timestamp::from_millis(1_794_387_600_000),
                sources: vec![],
                body: "Deploys go through `make ship`.\n\nNever by hand.".into(),
            }],
        };
        let text = memories(&ctx(&p, &d), &Some(Ok(notes))).plain();
        assert!(text.contains("Deploys · written 11 Nov 09:00:00"), "{text}");
        assert!(text.contains("  Deploys go through `make ship`."), "{text}");
        assert!(text.contains("  Never by hand."), "{text}");

        let skill = |scope, name: &str, version| SkillInfo {
            skill: SkillRef {
                scope,
                name: SkillName(name.into()),
            },
            version: SkillVersion(version),
            description: format!("When to use {name}"),
            body: String::new(),
        };
        let skills = Skills {
            skills: vec![
                skill(SkillScope::Shared, "deploy", 3),
                skill(SkillScope::Agent(agent), "triage", 1),
            ],
        };
        let text = super::skills(&ctx(&p, &d), agent, &Some(Ok(skills))).plain();
        let own = text.find("triage · version 1").expect("its own skill");
        let shared = text.find("deploy · version 3").expect("the shared skill");
        assert!(own < shared, "its own skills come first:\n{text}");
        assert!(text.contains("When to use deploy"), "{text}");

        let failed = memories(&ctx(&p, &d), &Some(Err("no repository".into()))).plain();
        assert!(
            failed.contains("could not be read: no repository"),
            "{failed}"
        );
    }
}
