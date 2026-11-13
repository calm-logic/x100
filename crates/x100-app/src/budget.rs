//! The Budget tab: spend against the project's budget, each lead's spend against its own, spend per
//! agent, tokens used, the lab's line, and which lines are stopped at their budget.

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::{BudgetLine, BudgetView, ProjectPart, ProjectState};
use x100_core::id::{AgentId, ProjectId};
use x100_core::money::Money;

use crate::session::{Session, SessionEvent};
use crate::{face, widgets};
use crate::{theme, words};
use x100_core::appearance::Appearance;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetModel {
    pub view: BudgetView,
    /// Every agent by name and look, the CEO first and then in the order they joined.
    pub agents: Vec<(AgentId, String, Appearance)>,
    pub ceo: AgentId,
}

impl BudgetModel {
    pub fn read(p: &ProjectState) -> Option<BudgetModel> {
        let view = p.budget()?;
        let mut agents: Vec<_> = p.agents().collect();
        let ceo = p.project()?.ceo;
        agents.sort_by_key(|a| (a.id != ceo, a.joined, a.role.clone()));
        Some(BudgetModel {
            ceo,
            view,
            agents: agents
                .into_iter()
                .map(|a| (a.id, a.role.clone(), a.appearance))
                .collect(),
        })
    }
}

pub struct BudgetTab {
    session: Entity<Session>,
    project: ProjectId,
    model: Option<BudgetModel>,
    _subscription: Subscription,
}

impl BudgetTab {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project
                        && (changes.parts.contains(&ProjectPart::Budget)
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
            model: None,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    pub fn shown(&self) -> Option<&BudgetModel> {
        self.model.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| BudgetModel::read(p))
            .flatten();
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }
}

fn share(spent: Money, of: Money) -> u8 {
    if of.micros() == 0 {
        return if spent.micros() > 0 { 100 } else { 0 };
    }
    (u128::from(spent.micros()) * 100 / u128::from(of.micros())).min(100) as u8
}

fn section(title: &'static str) -> impl IntoElement {
    div()
        .mt(px(10.))
        .text_size(px(theme::TINY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::dim())
        .child(title.to_uppercase())
}

/// A name, a meter, and what the line has spent, with a note when it is stopped.
/// A line's face is its agent's, and the lab, which is no agent, has none.
fn line(
    name: String,
    look: Option<Appearance>,
    meter: u8,
    amount: String,
    stopped: bool,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(6.))
        .text_size(px(theme::TEXT))
        .child(look.map_or_else(|| face::blank(22.), |l| face::face(22., l)))
        .child(div().flex_none().w(px(120.)).child(name))
        .child(div().flex_1().child(widgets::meter(meter)))
        .when(stopped, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(px(theme::TINY))
                    .text_color(theme::error())
                    .child("Stopped at its budget"),
            )
        })
        .child(
            div()
                .flex_none()
                .w(px(150.))
                .flex()
                .justify_end()
                .text_color(theme::body())
                .child(amount),
        )
}

fn of(line: &BudgetLine) -> String {
    format!(
        "{} of {}",
        words::money(line.spent),
        words::money(line.budget)
    )
}

impl Render for BudgetTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let page = div()
            .id("budget")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(16.));
        let Some(m) = self.model.clone() else {
            return page;
        };
        let v = &m.view;
        let max = v
            .agents
            .values()
            .map(|s| s.cost.micros())
            .max()
            .unwrap_or(0)
            .max(1);
        let name = |id: AgentId| {
            m.agents
                .iter()
                .find(|(a, ..)| *a == id)
                .map_or_else(|| "An agent".into(), |(_, n, _)| n.clone())
        };
        let look = |id: AgentId| m.agents.iter().find(|(a, ..)| *a == id).map(|(.., l)| *l);
        let u = &v.usage;
        page.child(
            div()
                .flex()
                .items_end()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(30.))
                        .child(words::money(v.project.spent)),
                )
                .child(
                    div()
                        .pb(px(5.))
                        .text_size(px(15.))
                        .text_color(theme::muted())
                        .child(format!(
                            "spent of this project's {} budget",
                            words::money(v.project.budget)
                        )),
                ),
        )
        .child(widgets::meter(share(v.project.spent, v.project.budget)).h(px(5.)))
        .when(v.project.stopped, |d| {
            d.child(
                div()
                    .text_size(px(theme::SMALL))
                    .text_color(theme::error())
                    .child("The project has reached its budget. No new turns start until it is raised."),
            )
        })
        .child(
            div()
                .mt(px(6.))
                .text_size(px(theme::TINY))
                .text_color(theme::dim())
                .child(format!(
                    "The budget is the most the team may spend on models for this whole project, a one-off total. {} tokens so far: {} in, {} out, {} read from cache, {} written to cache.",
                    words::tokens(u.all_input() + u.output),
                    words::tokens(u.input),
                    words::tokens(u.output),
                    words::tokens(u.cache_read),
                    words::tokens(u.cache_write_short + u.cache_write_long),
                )),
        )
        .child(section("Leads"))
        .children(v.leads.iter().map(|(id, l)| {
            line(
                name(*id),
                look(*id),
                share(l.spent, l.budget),
                of(l),
                l.stopped,
            )
        }))
        .when(v.leads.is_empty(), |d| {
            d.child(
                div()
                    .text_size(px(theme::SMALL))
                    .text_color(theme::dim())
                    .child("No leads yet."),
            )
        })
        .child(section("Lab"))
        .child(line(
            "Lab".into(),
            None,
            share(v.lab.spent, v.lab.budget),
            of(&v.lab),
            v.lab.stopped,
        ))
        .child(section("Spend by agent"))
        .children(m.agents.iter().map(|(id, role, look)| {
            let spent = v.agents.get(id).map_or(Money::ZERO, |s| s.cost);
            line(
                role.clone(),
                Some(*look),
                (u128::from(spent.micros()) * 100 / u128::from(max)) as u8,
                words::money(spent),
                false,
            )
        }))
    }
}
