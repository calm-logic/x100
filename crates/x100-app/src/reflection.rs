//! An agent's memories, skills and trials, as a small view of its own for the Team tab's view of
//! an agent: `Reflection::new(session, project, agent, cx)` makes it, and the Team tab places it
//! beside or under the agent's output. What exists comes from the folded state; the text of each
//! note and skill comes from `memories.list` and `skills.list`, and `trials.list` is asked too.
//! Where the daemon has not made one of those yet the section says so, and the names the state
//! holds still show.

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

/// What the state holds of the agent's lists, to know when to ask again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    notes: Vec<(String, String)>,
    skills: Vec<(SkillRef, u32)>,
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
        notice::run(
            cx,
            async move { client.list_trials(project).await },
            |this, r, cx| {
                this.trials = r.map_or_else(Fetch::Said, Fetch::Ready);
                cx.notify();
            },
        );
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

impl Render for Reflection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agent = self.agent;
        let (notes, skills, trials) = self
            .session
            .read(cx)
            .project(self.project, |p, _| {
                let a = p.agent(agent);
                (
                    a.map(|a| a.memories.keys().map(|n| n.0.clone()).collect::<Vec<_>>())
                        .unwrap_or_default(),
                    a.map(|a| a.skills.clone()).unwrap_or_default(),
                    p.trials_about(agent).cloned().collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();

        let memory_cards: Vec<_> = notes
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let body = match &self.memories {
                    Fetch::Ready(m) => m.notes.iter().find(|n| &n.name.0 == name).map(|n| {
                        let id: SharedString = format!("note-{i}").into();
                        AnyBody(Markdown::parse(&n.body).render(&id))
                    }),
                    _ => None,
                };
                card(name.clone(), "memory note".into(), body)
            })
            .collect();
        let skill_cards: Vec<_> = skills
            .iter()
            .enumerate()
            .map(|(i, loaded)| {
                let body = match &self.skills {
                    Fetch::Ready(s) => s.skills.iter().find(|k| k.skill == loaded.skill).map(|k| {
                        let id: SharedString = format!("skill-{i}").into();
                        AnyBody(Markdown::parse(&k.body).render(&id))
                    }),
                    _ => None,
                };
                let scope = match loaded.skill.scope {
                    SkillScope::Shared => "shared",
                    SkillScope::Agent(_) => "its own",
                };
                card(
                    loaded.skill.name.0.clone(),
                    format!("{scope} \u{b7} version {}", loaded.version.0),
                    body,
                )
            })
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
            .when(notes.is_empty(), |d| d.child(empty("No memory notes yet.")))
            .child(heading("Skills", self.skills.note()))
            .children(skill_cards)
            .when(skills.is_empty(), |d| d.child(empty("No skills loaded.")))
            .child(heading("Trials", self.trials.note()))
            .children(trial_cards)
            .when(trials.is_empty(), |d| {
                d.child(empty("No trials about this agent."))
            })
    }
}
