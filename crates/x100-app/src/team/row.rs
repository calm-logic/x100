//! One agent as the Team tab shows it: as a row under the agent that runs it, or as the header of
//! the view opened on it. Each is an entity of its own, drawn as a cached view, and redraws only
//! when what it shows has changed, so a change to one agent never redraws another.

use gpui::{Context, Entity, EventEmitter, FontWeight, Window, div, prelude::*, px};
use x100_core::agent::Position;
use x100_core::effort::{Effort, EffortRange};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{AgentId, ProjectId};

use crate::session::Session;
use crate::widgets::{self, Orb};
use crate::{theme, words};

pub const ROW_HEIGHT: f32 = 64.;
pub const HEADER_HEIGHT: f32 = 168.;

/// What a row shows, read from the folded state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentModel {
    pub name: String,
    pub ceo: bool,
    /// Its current task's title, or what it is doing without one.
    pub doing: String,
    pub progress: Option<u8>,
    /// The driver its task is running on: harness, model and effort.
    pub driver: Option<String>,
    pub driver_effort: Option<Effort>,
    /// Its effort range: its own, or for a worker without one its parent's.
    pub range: EffortRange,
    /// How many agents it runs.
    pub agents: usize,
}

impl AgentModel {
    pub fn read(p: &ProjectState, daemon: &DaemonState, agent: AgentId) -> Option<AgentModel> {
        let row = p.row(agent)?;
        let doing = match row.task {
            Some(task) => task.task.title.clone(),
            None => words::agent_state(&row.agent.state).to_owned(),
        };
        let driver = row.driver.map(|d| {
            format!(
                "{} · {} · {}",
                words::harness(daemon, d.driver.harness),
                d.driver.model.0,
                words::effort(d.driver.effort)
            )
        });
        Some(AgentModel {
            name: row.agent.role.clone(),
            ceo: matches!(row.position, Position::Ceo { .. }),
            doing,
            progress: row.progress.map(|p| p.value()),
            driver,
            driver_effort: row.driver.map(|d| d.driver.effort),
            range: row.effort,
            agents: p.children(agent).len(),
        })
    }
}

pub enum RowEvent {
    Open(AgentId),
}

/// An agent under the one the view is opened on. Clicking it opens it.
pub struct AgentRow {
    session: Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    renders: usize,
    model: Option<AgentModel>,
}

impl EventEmitter<RowEvent> for AgentRow {}

/// Reads what `agent` shows now, and asks for a redraw if that differs from `model`.
fn refresh<V: 'static>(
    session: &Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    model: &mut Option<AgentModel>,
    cx: &mut Context<V>,
) {
    let now = session
        .read(cx)
        .project(project, |p, d| AgentModel::read(p, d, agent))
        .flatten();
    if now != *model {
        *model = now;
        cx.notify();
    }
}

impl AgentRow {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut row = Self {
            session,
            project,
            agent,
            renders: 0,
            model: None,
        };
        row.refresh(cx);
        row
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        refresh(&self.session, self.project, self.agent, &mut self.model, cx);
    }

    pub fn agent(&self) -> AgentId {
        self.agent
    }

    /// How many times it has been drawn, to prove that unrelated changes leave it alone.
    pub fn renders(&self) -> usize {
        self.renders
    }

    /// What it shows.
    pub fn shown(&self) -> Option<&AgentModel> {
        self.model.as_ref()
    }
}

impl Render for AgentRow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let Some(m) = self.model.clone() else {
            return div().into_any_element();
        };
        let detail = if m.agents > 0 {
            format!("{} · {}", m.doing, words::agents(m.agents))
        } else {
            m.doing.clone()
        };
        div()
            .id("agent-row")
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .rounded(px(12.))
            .border_1()
            .border_color(theme::line())
            .cursor_pointer()
            .hover(|s| s.border_color(gpui::rgba(0xffffff80)).bg(theme::hover()))
            .on_click(cx.listener(|this, _, _, cx| cx.emit(RowEvent::Open(this.agent))))
            .child(widgets::orb(40., Orb::Agent))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(theme::ROW_TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(m.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(theme::SMALL))
                            .text_color(theme::muted())
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(detail),
                    )
                    .child(
                        div()
                            .mt(px(5.))
                            .child(widgets::meter(m.progress.unwrap_or(0))),
                    ),
            )
            .child(driver(&m))
            .into_any_element()
    }
}

/// The driver pill over the effort pips, right-aligned.
fn driver(m: &AgentModel) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(6.))
        .child(widgets::pill(
            m.driver.clone().unwrap_or_else(|| "No driver".into()),
        ))
        .child(widgets::pips(m.range, m.driver_effort))
}

/// The agent a view is opened on, large and centred: the CEO until another is opened.
pub struct AgentHeader {
    session: Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    model: Option<AgentModel>,
}

impl AgentHeader {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut header = Self {
            session,
            project,
            agent,
            model: None,
        };
        header.refresh(cx);
        header
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        refresh(&self.session, self.project, self.agent, &mut self.model, cx);
    }

    pub fn agent(&self) -> AgentId {
        self.agent
    }

    /// What it shows.
    pub fn shown(&self) -> Option<&AgentModel> {
        self.model.as_ref()
    }
}

impl Render for AgentHeader {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Some(m) = self.model.clone() else {
            return div().into_any_element();
        };
        let size = if m.ceo { 76. } else { 52. };
        div()
            .h(px(HEADER_HEIGHT))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(px(3.))
            .text_size(px(theme::ROW_TEXT))
            .child(widgets::orb(size, if m.ceo { Orb::Ceo } else { Orb::Agent }).mb(px(6.)))
            // The CEO's orb says who it is; another agent's name says which one is open.
            .when(!m.ceo, |d| {
                d.child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(m.name.clone()),
                )
            })
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .text_color(theme::muted())
                    .child(m.doing.clone()),
            )
            .when_some(m.driver.clone(), |d, label| {
                d.child(
                    div()
                        .mt(px(8.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(6.))
                        .child(widgets::pill(label))
                        .child(widgets::pips(m.range, m.driver_effort)),
                )
            })
            .into_any_element()
    }
}
