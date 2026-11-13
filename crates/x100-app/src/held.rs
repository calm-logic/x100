//! Held actions: what is waiting for the builder, why it was flagged, and Approve and Deny. The
//! app mock shows no place for them, so they sit as a strip above the chat's composer, where the
//! builder is already looking when an agent needs an answer.

use std::collections::HashMap;

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::action::{Decision, Risk};
use x100_core::fold::{ProjectPart, ProjectState};
use x100_core::id::{ActionId, ProjectId};

use crate::glass;
use crate::log::{self, Names};
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{face, theme, widgets, words};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldModel {
    pub id: ActionId,
    pub agent: String,
    pub look: x100_core::appearance::Appearance,
    /// What it is, as a phrase after the agent's name: "run `rm -rf build`".
    pub what: String,
    /// The kind of action, when it has been classified.
    pub kind: String,
    pub risk: Option<Risk>,
    pub why: String,
}

impl HeldModel {
    pub fn all(p: &ProjectState, daemon: &x100_core::fold::DaemonState) -> Vec<HeldModel> {
        let files = HashMap::new();
        let names = Names {
            project: p,
            daemon,
            files: &files,
        };
        let mut held: Vec<_> = p.held_actions().collect();
        held.sort_by_key(|a| (a.proposed, a.action.id));
        held.into_iter()
            .map(|a| HeldModel {
                id: a.action.id,
                agent: p
                    .agent(a.action.agent)
                    .map_or_else(|| "An agent".into(), |r| r.role.clone()),
                look: p
                    .agent(a.action.agent)
                    .map(|r| r.appearance)
                    .unwrap_or_default(),
                what: log::request(&names, &a.action.request),
                kind: a
                    .classification
                    .as_ref()
                    .map(|c| log::kind(c.kind).to_owned())
                    .unwrap_or_default(),
                risk: a.classification.as_ref().map(|c| c.risk),
                why: a
                    .classification
                    .as_ref()
                    .map_or_else(String::new, |c| c.why.clone()),
            })
            .collect()
    }
}

pub struct HeldStrip {
    session: Entity<Session>,
    project: ProjectId,
    held: Vec<HeldModel>,
    outcomes: HashMap<ActionId, Outcome>,
    _subscription: Subscription,
}

impl HeldStrip {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project
                        && (changes.parts.contains(&ProjectPart::HeldActions)
                            || changes.parts.contains(&ProjectPart::Looks))
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
            held: Vec::new(),
            outcomes: HashMap::new(),
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// The actions waiting, oldest first.
    pub fn shown(&self) -> &[HeldModel] {
        &self.held
    }

    pub fn outcome(&self, action: ActionId) -> Option<&Outcome> {
        self.outcomes.get(&action)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .project(self.project, HeldModel::all)
            .unwrap_or_default();
        if now != self.held {
            self.outcomes
                .retain(|id, _| now.iter().any(|h| h.id == *id));
            self.held = now;
            cx.notify();
        }
    }

    pub fn decide(&mut self, action: ActionId, decision: Decision, cx: &mut Context<Self>) {
        let client = self.session.read(cx).client().clone();
        let project = self.project;
        self.outcomes.insert(action, Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.decide_action(project, action, decision).await },
            move |this, result, cx| {
                match result {
                    // The action leaves the strip when the state says it is no longer held.
                    Ok(()) => this.outcomes.remove(&action),
                    Err(outcome) => this.outcomes.insert(action, outcome),
                };
                cx.notify();
            },
        );
    }
}

impl Render for HeldStrip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cards: Vec<_> = self
            .held
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let id = h.id;
                let risk = h.risk.map(words::risk);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme::ask())
                    .bg(theme::raised())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(face::face(18., h.look))
                            .child(
                                div()
                                    .text_size(px(theme::TEXT))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{} wants to {}", h.agent, h.what)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TINY))
                            .text_color(theme::ask())
                            .child(match risk {
                                Some(risk) => format!("Flagged: {risk}, {}", h.kind),
                                None => "Flagged".into(),
                            }),
                    )
                    .when(!h.why.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(theme::SMALL))
                                .text_color(theme::muted())
                                .child(h.why.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                glass::primary(("approve", i), "Approve")
                                    .selector(format!("approve-{i}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.decide(id, Decision::Approved, cx)
                                    })),
                            )
                            .child(
                                widgets::button(("deny", i), "Deny")
                                    .debug_selector(move || format!("deny-{i}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.decide(id, Decision::Denied, cx)
                                    })),
                            )
                            .children(self.outcomes.get(&id).map(notice::line)),
                    )
            })
            .collect();
        div()
            .debug_selector(|| "held-actions".into())
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(!cards.is_empty(), |d| d.px(px(14.)).pt(px(12.)))
            .children(cards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x100_core::fold::DaemonState;
    use x100_core::testing::{daemon_log, fernleaf};

    /// While an action waits, the strip shows what it is, why it was flagged, and its risk.
    #[test]
    fn a_held_action_shows_what_it_is_and_why_it_was_flagged() {
        let f = fernleaf();
        let daemon = DaemonState::fold(&daemon_log(x100_core::testing::START)).unwrap();
        let mut state = ProjectState::default();
        let mut shown = None;
        for e in &f.log {
            state.apply(e).unwrap();
            let held = HeldModel::all(&state, &daemon);
            if let Some(h) = held.iter().find(|h| h.id == f.deploy) {
                shown = Some(h.clone());
                break;
            }
        }
        let h = shown.expect("the deploy waits for the builder at some point");
        assert!(!h.agent.is_empty() && !h.what.is_empty(), "{h:?}");
        assert!(h.risk.is_some(), "{h:?}");
        assert!(!h.why.is_empty(), "{h:?}");
        // Once decided, it is not in the strip.
        for e in f.log.iter().skip(state.seq().unwrap().0 as usize) {
            state.apply(e).unwrap();
        }
        assert!(
            HeldModel::all(&state, &daemon)
                .iter()
                .all(|h| h.id != f.deploy)
        );
    }
}
