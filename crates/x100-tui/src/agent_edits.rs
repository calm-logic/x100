//! What the builder changes from an agent's page: its look, and a skill, new or rewritten. A skill
//! is written as its `SKILL.md`: front matter naming it and saying when to use it, then what it
//! says. The daemon sets its version.

use x100_core::appearance::{Appearance, Outline};
use x100_core::id::{AgentId, ProjectId};
use x100_core::memory::{SkillName, SkillRef, SkillScope};

use crate::app::{App, Then};
use crate::look;
use crate::msg::Msg;

/// A prompt's purpose on an agent's page.
#[derive(Debug, Clone)]
pub enum AgentAsked {
    Outline(AgentId),
    Hue {
        agent: AgentId,
        outline: Outline,
    },
    SkillName(AgentId),
    SkillShared {
        agent: AgentId,
        name: String,
    },
    SkillAbout {
        agent: AgentId,
        skill: SkillRef,
    },
    SkillText {
        agent: AgentId,
        skill: SkillRef,
        about: String,
    },
}

impl App {
    fn look_of(&self, project: ProjectId, agent: AgentId) -> Appearance {
        self.client
            .project(project)
            .and_then(|p| p.agent(agent).map(|a| a.appearance))
            .unwrap_or_default()
    }

    /// `l` on an agent's page: its outline, then its colour.
    pub(crate) fn ask_look(&mut self, project: ProjectId, agent: AgentId) {
        let now = self.look_of(project, agent);
        self.ask(
            &format!(
                "Outline (now {}; Enter keeps it): {}",
                x100_client::words::outline(now.outline),
                look::outline_choices()
            ),
            Then::Agent(AgentAsked::Outline(agent)),
        );
    }

    /// `n` on its skills: a new skill.
    pub(crate) fn ask_new_skill(&mut self, agent: AgentId) {
        self.ask(
            "Name of the skill, such as deploy",
            Then::Agent(AgentAsked::SkillName(agent)),
        );
    }

    /// `e` on one of its skills: the same skill rewritten, its words to start from.
    pub(crate) fn ask_edit_skill(&mut self, agent: AgentId, skill: SkillRef) {
        let about = self
            .skill_text(agent, &skill)
            .map(|(about, _)| about)
            .unwrap_or_default();
        self.ask_with(
            &format!("When to use {}", skill.name.0),
            Then::Agent(AgentAsked::SkillAbout { agent, skill }),
            &about,
        );
    }

    /// The description and the instructions of a skill the page has read.
    fn skill_text(&self, agent: AgentId, skill: &SkillRef) -> Option<(String, String)> {
        let a = self
            .project_ui()?
            .agent
            .as_ref()
            .filter(|a| a.id == agent)?;
        let k = a
            .skills
            .as_ref()?
            .as_ref()
            .ok()?
            .skills
            .iter()
            .find(|k| k.skill == *skill)?;
        Some((k.description.clone(), instructions(&k.body).to_owned()))
    }

    pub(crate) fn agent_answered(&mut self, project: ProjectId, asked: AgentAsked, text: String) {
        match asked {
            AgentAsked::Outline(agent) => {
                let outline = if text.is_empty() {
                    Some(self.look_of(project, agent).outline)
                } else {
                    look::parse_outline(&text)
                };
                let Some(outline) = outline else {
                    return self.fail("Choose an outline by its number, 1 to 8");
                };
                let now = self.look_of(project, agent);
                self.ask(
                    &format!(
                        "Colour (now {}; Enter keeps it): {}",
                        x100_client::words::hue(now.hue),
                        look::hue_choices()
                    ),
                    Then::Agent(AgentAsked::Hue { agent, outline }),
                );
            }
            AgentAsked::Hue { agent, outline } => {
                let hue = if text.is_empty() {
                    Some(self.look_of(project, agent).hue)
                } else {
                    look::parse_hue(&text)
                };
                let Some(hue) = hue else {
                    return self.fail("Choose a colour by its number, 0 to 8");
                };
                let client = self.client.clone();
                let appearance = Appearance { outline, hue };
                self.spawn(async move {
                    client.set_appearance(project, agent, appearance).await?;
                    Ok(format!(
                        "It is {} now",
                        x100_client::words::look(appearance)
                    ))
                });
            }
            AgentAsked::SkillName(_) if text.is_empty() => {}
            AgentAsked::SkillName(agent) => self.ask(
                "Only for this agent, or shared with every agent? (a/s)",
                Then::Agent(AgentAsked::SkillShared { agent, name: text }),
            ),
            AgentAsked::SkillShared { agent, name } => {
                let scope = match text.to_ascii_lowercase().as_str() {
                    "a" | "agent" => SkillScope::Agent(agent),
                    "s" | "shared" => SkillScope::Shared,
                    _ => return self.fail("Answer a for this agent alone, or s for shared"),
                };
                let skill = SkillRef {
                    scope,
                    name: SkillName(name),
                };
                self.ask(
                    &format!("When to use {}", skill.name.0),
                    Then::Agent(AgentAsked::SkillAbout { agent, skill }),
                );
            }
            AgentAsked::SkillAbout { .. } if text.is_empty() => {
                self.fail("A skill says when to use it")
            }
            AgentAsked::SkillAbout { agent, skill } => {
                let body = self
                    .skill_text(agent, &skill)
                    .map(|(_, body)| body)
                    .unwrap_or_default();
                self.ask_with(
                    "What it says (Alt-Enter starts a new line)",
                    Then::Agent(AgentAsked::SkillText {
                        agent,
                        skill,
                        about: text,
                    }),
                    &body,
                );
            }
            AgentAsked::SkillText {
                agent,
                skill,
                about,
            } => self.write_skill(project, agent, skill, &about, &text),
        }
    }

    /// Writes the skill, then reads the agent's skills again so its page shows the new version.
    fn write_skill(
        &mut self,
        project: ProjectId,
        agent: AgentId,
        skill: SkillRef,
        about: &str,
        body: &str,
    ) {
        let text = skill_md(&skill.name.0, about, body);
        let (client, tx) = (self.client.clone(), self.tx.clone());
        self.handle.spawn(async move {
            let name = skill.name.0.clone();
            let notice = match client.write_skill(project, skill, text).await {
                Ok(w) => format!("Wrote the skill {name}, version {}", w.version.0),
                Err(e) => format!("! {e}"),
            };
            let result = client.list_skills(project, None).await;
            let _ = tx.send(Msg::Skills { agent, result });
            let _ = tx.send(Msg::Notice(notice));
        });
    }
}

/// A `SKILL.md`. The description is kept on one line, as front matter needs.
pub fn skill_md(name: &str, about: &str, body: &str) -> String {
    let about = about.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "---\nname: {name}\ndescription: {about}\n---\n\n{}\n",
        body.trim_end()
    )
}

/// What a `SKILL.md` says after its front matter.
pub fn instructions(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text.trim();
    };
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            after.trim()
        }
        None => text.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skill_is_written_with_its_front_matter_and_read_back_without_it() {
        let text = skill_md(
            "deploy",
            "How to\ndeploy  it.",
            "Run `make ship`.\nThen look.\n",
        );
        assert_eq!(
            text,
            "---\nname: deploy\ndescription: How to deploy it.\n---\n\nRun `make ship`.\nThen look.\n"
        );
        let stored = "---\nname: deploy\ndescription: How to deploy it.\nversion: 3\n---\n\nRun `make ship`.\nThen look.\n";
        assert_eq!(instructions(stored), "Run `make ship`.\nThen look.");
        assert_eq!(instructions("no front matter"), "no front matter");
    }
}
