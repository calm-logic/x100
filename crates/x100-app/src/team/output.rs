//! What an agent that runs no agents of its own is working in: its driver's live terminal and
//! tool output, continuous across a change of driver.
//!
//! **The seam for M15b.** This is a placeholder. The Team tab creates one `AgentOutput` for the
//! agent it is opened on, when that agent has no agents under it, and lays it out to fill the rest
//! of the view; it does nothing else with it. M15b replaces the body of this module with the live
//! view (`Client::follow_terminal` for a terminal driver, `Client::follow_transcript` for a
//! headless one) and keeps `AgentOutput::new`'s signature, so the Team tab does not change.

use gpui::{Context, Entity, Window, div, prelude::*, px};
use x100_core::id::{AgentId, ProjectId};

use crate::session::Session;
use crate::theme;

pub struct AgentOutput {
    session: Entity<Session>,
    project: ProjectId,
    agent: AgentId,
}

impl AgentOutput {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            session,
            project,
            agent,
        }
    }

    pub fn agent(&self) -> AgentId {
        self.agent
    }
}

impl Render for AgentOutput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agent = self.agent;
        let role = self
            .session
            .read(cx)
            .project(self.project, |p, _| p.agent(agent).map(|a| a.role.clone()))
            .flatten()
            .unwrap_or_default();
        // Drawn like the mock's screen: a bar, then the output.
        div()
            .size_full()
            .min_h(px(190.))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(theme::line())
            .bg(gpui::rgb(0x0a0a0b))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(theme::line())
                    .bg(gpui::rgb(0x161617))
                    .font_family(theme::MONO)
                    .text_size(px(theme::TINY))
                    .text_color(theme::muted())
                    .children((0..3).map(|_| crate::widgets::dot(9., gpui::rgb(0x3a3a3d))))
                    .child(div().ml(px(6.)).child(format!("{role} · live output"))),
            )
            .child(
                div()
                    .flex_1()
                    .p(px(14.))
                    .font_family(theme::MONO)
                    .text_size(px(theme::SMALL))
                    .text_color(theme::dim())
                    .child("The live output of this agent's driver will show here."),
            )
    }
}
