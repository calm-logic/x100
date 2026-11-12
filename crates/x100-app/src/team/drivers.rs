//! The Team tab's other listing: every live and warm driver in the project, one cached row each.

use gpui::{Context, Entity, FontWeight, Window, div, prelude::*, px};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{DriverId, ProjectId};

use crate::session::Session;
use crate::{theme, words};

pub const DRIVER_ROW_HEIGHT: f32 = 38.;

/// The columns, with each one's share of the width.
pub const COLUMNS: [(&str, f32); 8] = [
    ("Agent", 1.3),
    ("Harness", 1.2),
    ("Model", 1.2),
    ("Effort", 0.8),
    ("Machine", 1.0),
    ("State", 0.9),
    ("Context", 0.8),
    ("Cost", 0.8),
];

/// What a driver's row shows, in the order of [`COLUMNS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverModel(pub [String; 8]);

impl DriverModel {
    pub fn read(p: &ProjectState, daemon: &DaemonState, id: DriverId) -> Option<DriverModel> {
        let d = p.driver(id)?;
        let agent = p
            .agent(d.driver.agent)
            .map_or_else(|| "Unknown agent".into(), |a| a.role.clone());
        Some(DriverModel([
            agent,
            words::harness(daemon, d.driver.harness),
            d.driver.model.0.clone(),
            words::effort(d.driver.effort).into(),
            words::machine(daemon, d.driver.machine),
            words::driver_state(&d.state).into(),
            words::context(d.context),
            words::money(d.spent.cost),
        ]))
    }
}

pub struct DriverRow {
    session: Entity<Session>,
    project: ProjectId,
    driver: DriverId,
    renders: usize,
    model: Option<DriverModel>,
}

impl DriverRow {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        driver: DriverId,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut row = Self {
            session,
            project,
            driver,
            renders: 0,
            model: None,
        };
        row.refresh(cx);
        row
    }

    /// Reads what the driver shows now, and redraws only if that changed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let driver = self.driver;
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, d| DriverModel::read(p, d, driver))
            .flatten();
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn shown(&self) -> Option<&DriverModel> {
        self.model.as_ref()
    }
}

impl Render for DriverRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let cells = self.model.clone().map(|m| m.0).unwrap_or_default();
        line(cells, false)
    }
}

/// A row of the listing, or its head.
pub fn line(cells: [String; 8], head: bool) -> gpui::Div {
    div()
        .h(px(DRIVER_ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(16.))
        .border_b_1()
        .border_color(theme::line())
        .text_size(px(if head { 9. } else { theme::SMALL }))
        .text_color(if head { theme::dim() } else { theme::muted() })
        .children(
            cells
                .into_iter()
                .zip(COLUMNS)
                .enumerate()
                .map(|(i, (cell, (_, share)))| {
                    div()
                        .flex_grow(share)
                        .flex_basis(px(0.))
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .when(i == 0 && !head, |d| {
                            d.text_color(theme::text()).font_weight(FontWeight::MEDIUM)
                        })
                        .child(if head { cell.to_uppercase() } else { cell })
                }),
        )
}
