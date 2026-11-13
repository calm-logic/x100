//! An agent's memories, skills and trials, as a small view of its own for the Team tab's view of
//! an agent: `Reflection::new(session, project, agent, cx)` makes it, and the Team tab places it
//! beside or under the agent's output. The notes and skills are what the repositories hold, from
//! `memories.list` and `skills.list`, asked again whenever the folded state says one changed; the
//! state's own record of them only says when to ask, and stands in for the notes' names while the
//! answer is on its way. Trials are not served until the lab is built (M14), so they are neither
//! asked for nor shown.

use gpui::{Context, Entity, FontWeight, SharedString, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::{ProjectPart, ProjectState, TrialRecord};
use x100_core::id::{AgentId, ProjectId};
use x100_core::memory::{SkillRef, SkillScope};
use x100_core::trial::{Arm, TrialChange, TrialState, Verdict};
use x100_proto::{Memories, Skills, Trials};

use crate::markdown::Markdown;
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, words};

/// What a call has given so far.
enum Fetch<T> {
    Loading,
    Ready(T),
    Said(Outcome),
}

impl<T> Fetch<T> {
    fn note(&self) -> Option<&Outcome> {
        match self {
            Fetch::Said(o) => Some(o),
            _ => None,
        }
    }
}

/// Whether the daemon serves trials. It does not until M14 builds the lab, and until then the
/// agent view leaves them out rather than say so.
const TRIALS_SERVED: bool = false;

/// What the state holds of the agent's lists, to know when to ask again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    notes: Vec<(String, String)>,
    /// The skills the agent has loaded, and every skill change that reached a main branch.
    skills: Vec<(SkillRef, u32)>,
    committed: Vec<(SkillRef, u32, String)>,
    trials: Vec<(String, TrialState)>,
}

impl Signature {
    fn read(p: &ProjectState, agent: AgentId) -> Option<Signature> {
        let a = p.agent(agent)?;
        Some(Signature {
            notes: a
                .memories
                .iter()
                .map(|(n, c)| (n.0.clone(), c.0.clone()))
                .collect(),
            skills: a
                .skills
                .iter()
                .map(|s| (s.skill.clone(), s.version.0))
                .collect(),
            committed: p
                .skills()
                .iter()
                .map(|s| (s.skill.clone(), s.version.0, s.commit.0.clone()))
                .collect(),
            trials: p
                .trials_about(agent)
                .map(|t| (t.trial.id.to_string(), t.state))
                .collect(),
        })
    }
}

pub struct Reflection {
    session: Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    seen: Option<Signature>,
    memories: Fetch<Memories>,
    skills: Fetch<Skills>,
    trials: Fetch<Trials>,
    _subscription: Subscription,
}

impl Reflection {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project && changes.parts.contains(&ProjectPart::Team)
                }
                SessionEvent::Changed(Change::Reset) => true,
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let mut view = Self {
            session,
            project,
            agent,
            seen: None,
            memories: Fetch::Loading,
            skills: Fetch::Loading,
            trials: Fetch::Loading,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// What each call answered: `None` while it is loading, `Some(None)` when it gave its lists
    /// and `Some(Some(outcome))` when it did not.
    pub fn notes(&self) -> [Option<Option<&Outcome>>; 3] {
        fn of<T>(f: &Fetch<T>) -> Option<Option<&Outcome>> {
            match f {
                Fetch::Loading => None,
                Fetch::Ready(_) => Some(None),
                Fetch::Said(o) => Some(Some(o)),
            }
        }
        [of(&self.memories), of(&self.skills), of(&self.trials)]
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let agent = self.agent;
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| Signature::read(p, agent))
            .flatten();
        if now == self.seen {
            return;
        }
        self.seen = now;
        let (client, project) = (self.session.read(cx).client().clone(), self.project);
        let c = client.clone();
        notice::run(
            cx,
            async move { c.list_memories(project, agent).await },
            |this, r, cx| {
                this.memories = r.map_or_else(Fetch::Said, Fetch::Ready);
                cx.notify();
            },
        );
        let c = client.clone();
        notice::run(
            cx,
            async move { c.list_skills(project, None).await },
            |this, r, cx| {
                this.skills = r.map_or_else(Fetch::Said, Fetch::Ready);
                cx.notify();
            },
        );
        if TRIALS_SERVED {
            notice::run(
                cx,
                async move { client.list_trials(project).await },
                |this, r, cx| {
                    this.trials = r.map_or_else(Fetch::Said, Fetch::Ready);
                    cx.notify();
                },
            );
        }
        cx.notify();
    }
}

fn heading(title: &'static str, note: Option<&Outcome>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .mt(px(10.))
        .child(
            div()
                .text_size(px(theme::TINY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::dim())
                .child(title.to_uppercase()),
        )
        .children(note.map(notice::line))
}

fn card(title: String, subtitle: String, body: Option<AnyBody>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .p(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(theme::line())
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(theme::TEXT))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(theme::TINY))
                        .text_color(theme::muted())
                        .child(subtitle),
                ),
        )
        .children(body.map(|b| {
            div()
                .text_size(px(theme::SMALL))
                .text_color(theme::body())
                .child(b.0)
        }))
}

struct AnyBody(gpui::AnyElement);

fn empty(text: &'static str) -> impl IntoElement {
    div()
        .text_size(px(theme::SMALL))
        .text_color(theme::dim())
        .child(text)
}

fn arm_line(t: &TrialRecord, arm: Arm) -> String {
    let runs: Vec<_> = t.runs.iter().filter(|r| r.arm == arm).collect();
    if runs.is_empty() {
        return "no runs yet".into();
    }
    let passed: u32 = runs.iter().map(|r| r.checks_passed).sum();
    let total: u32 = runs.iter().map(|r| r.checks_total).sum();
    let cost = runs
        .iter()
        .fold(x100_core::money::Money::ZERO, |a, r| a + r.cost);
    format!(
        "{passed} of {total} checks over {} runs, {}",
        runs.len(),
        words::money(cost)
    )
}

fn trial_change(change: &TrialChange) -> String {
    match change {
        TrialChange::EditSkill { skill, .. } => format!("A change to the skill {}", skill.name.0),
        TrialChange::NewSkill { skill, .. } => format!("A new skill, {}", skill.name.0),
        TrialChange::CompactMemory { .. } => "Compacting its memory".into(),
    }
}

fn trial_state(state: TrialState) -> &'static str {
    match state {
        TrialState::Proposed => "Proposed",
        TrialState::Running => "Running",
        TrialState::Scored(Verdict::Supports) => "Scored: supports the change",
        TrialState::Scored(Verdict::DoesNotSupport) => "Scored: does not support the change",
        TrialState::Committed => "Committed",
        TrialState::Discarded => "Dropped",
        TrialState::Stopped => "Stopped",
    }
}

/// One card: a note or a skill as the view lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    pub subtitle: String,
    /// The Markdown shown under it, once the daemon has answered.
    pub body: Option<String>,
}

/// What the view lists: the agent's notes and the skills it can load.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listed {
    pub notes: Vec<Card>,
    pub skills: Vec<Card>,
}

/// A skill's text without its front matter, which the card's subtitle says the useful part of.
fn without_front_matter(text: &str) -> &str {
    text.strip_prefix("---\n")
        .and_then(|rest| {
            rest.split_once("\n---\n")
                .map(|(_, body)| body.trim_start())
        })
        .unwrap_or(text)
}

impl Reflection {
    /// The cards, from the daemon's answers where it has given them and from the state's record
    /// until then.
    pub fn listed(&self, cx: &gpui::App) -> Listed {
        let agent = self.agent;
        let (names, loaded) = self
            .session
            .read(cx)
            .project(self.project, |p, _| {
                let a = p.agent(agent);
                (
                    a.map(|a| a.memories.keys().map(|n| n.0.clone()).collect::<Vec<_>>())
                        .unwrap_or_default(),
                    a.map(|a| a.skills.clone()).unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let notes = match &self.memories {
            Fetch::Ready(m) => m
                .notes
                .iter()
                .map(|n| Card {
                    name: n.name.0.clone(),
                    subtitle: match n.sources.len() {
                        0 => "memory note".into(),
                        1 => "memory note \u{b7} cites 1 event".into(),
                        k => format!("memory note \u{b7} cites {k} events"),
                    },
                    body: Some(n.body.clone()),
                })
                .collect(),
            _ => names
                .into_iter()
                .map(|name| Card {
                    name,
                    subtitle: "memory note".into(),
                    body: None,
                })
                .collect(),
        };
        let scope = |s: &SkillRef| match s.scope {
            SkillScope::Shared => "shared",
            SkillScope::Agent(_) => "its own",
        };
        let skills = match &self.skills {
            Fetch::Ready(all) => all
                .skills
                .iter()
                .filter(|k| match k.skill.scope {
                    SkillScope::Shared => true,
                    SkillScope::Agent(a) => a == agent,
                })
                .map(|k| {
                    let used = loaded.iter().any(|l| l.skill == k.skill);
                    Card {
                        name: k.skill.name.0.clone(),
                        subtitle: format!(
                            "{} \u{b7} version {}{}",
                            scope(&k.skill),
                            k.version.0,
                            if used { " \u{b7} loaded" } else { "" }
                        ),
                        body: Some(format!(
                            "{}\n\n{}",
                            k.description,
                            without_front_matter(&k.body)
                        )),
                    }
                })
                .collect(),
            _ => loaded
                .iter()
                .map(|l: &x100_core::fold::LoadedSkill| Card {
                    name: l.skill.name.0.clone(),
                    subtitle: format!("{} \u{b7} version {}", scope(&l.skill), l.version.0),
                    body: None,
                })
                .collect(),
        };
        Listed { notes, skills }
    }
}

impl Render for Reflection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agent = self.agent;
        let listed = self.listed(cx);
        let trials = if TRIALS_SERVED {
            self.session
                .read(cx)
                .project(self.project, |p, _| {
                    p.trials_about(agent).cloned().collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let card_of = |prefix: &str, i: usize, c: &Card| {
            let body = c.body.as_ref().map(|text| {
                let id: SharedString = format!("{prefix}-{i}").into();
                AnyBody(Markdown::parse(text).render(&id))
            });
            card(c.name.clone(), c.subtitle.clone(), body)
        };
        let memory_cards: Vec<_> = listed
            .notes
            .iter()
            .enumerate()
            .map(|(i, c)| card_of("note", i, c))
            .collect();
        let skill_cards: Vec<_> = listed
            .skills
            .iter()
            .enumerate()
            .map(|(i, c)| card_of("skill", i, c))
            .collect();
        let trial_cards: Vec<_> = trials
            .iter()
            .map(|t| {
                card(
                    trial_change(&t.trial.change),
                    trial_state(t.state).into(),
                    Some(AnyBody(
                        div()
                            .flex()
                            .flex_col()
                            .child(format!(
                                "Without the change: {}",
                                arm_line(t, Arm::Baseline)
                            ))
                            .child(format!("With the change: {}", arm_line(t, Arm::Changed)))
                            .into_any_element(),
                    )),
                )
            })
            .collect();

        div()
            .id("reflection")
            .debug_selector(|| "reflection".into())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(heading("Memories", self.memories.note()))
            .children(memory_cards)
            .when(listed.notes.is_empty(), |d| {
                d.child(empty("No memory notes yet."))
            })
            .child(heading("Skills", self.skills.note()))
            .children(skill_cards)
            .when(listed.skills.is_empty(), |d| {
                d.child(empty("No skills yet."))
            })
            .when(TRIALS_SERVED, |d| {
                d.child(heading("Trials", self.trials.note()))
                    .children(trial_cards)
                    .when(trials.is_empty(), |d| {
                        d.child(empty("No trials about this agent."))
                    })
            })
    }
}
