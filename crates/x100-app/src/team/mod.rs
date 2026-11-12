//! The Team tab: the agent it is opened on (the CEO to begin with), a trail back to the CEO, and
//! either the agents it runs or what it is working in. A switch lists the project's live drivers
//! instead.
//!
//! The view itself is not cached and redraws cheaply whenever anything under it does. Each agent
//! and each driver is a cached view of its own. When an event changes the Team part (core's
//! `Changes`), each reads what it shows again and redraws only if that differs, so one agent's
//! progress never redraws another's row.

mod drivers;
mod facts;
mod log;
mod output;
mod row;

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Entity, FontWeight, StyleRefinement, Subscription, Window, div,
    prelude::*, px,
};
use x100_client::Change;
use x100_core::fold::{ProjectPart, ProjectState};
use x100_core::id::{AgentId, DriverId, ProjectId};

pub use drivers::{DriverModel, DriverRow};
pub use facts::{AgentFacts, FactsModel};
pub use output::{AgentOutput, SegmentShown};
pub use row::{AgentHeader, AgentModel, AgentRow, RowEvent};

use crate::reflection::Reflection;
use crate::session::{Session, SessionEvent};
use crate::{theme, widgets};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    Agents,
    Drivers,
}

pub struct TeamView {
    session: Entity<Session>,
    project: ProjectId,
    listing: Listing,
    /// The agent the view is opened on; `None` is the CEO.
    focus: Option<AgentId>,
    header: Option<Entity<AgentHeader>>,
    /// Every row made so far, kept so a row seen again keeps its entity.
    rows: HashMap<AgentId, Entity<AgentRow>>,
    /// The agents listed now, under the one the view is opened on.
    listed: Vec<AgentId>,
    drivers: HashMap<DriverId, Entity<DriverRow>>,
    listed_drivers: Vec<DriverId>,
    output: Option<Entity<AgentOutput>>,
    /// The agent's memories, skills and trials, under its output.
    reflection: Option<Entity<Reflection>>,
    _subscriptions: Vec<Subscription>,
}

impl TeamView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            if let SessionEvent::Changed(change) = event {
                this.changed(change, cx);
            }
        });
        let mut view = Self {
            session,
            project,
            listing: Listing::Agents,
            focus: None,
            header: None,
            rows: HashMap::new(),
            listed: Vec::new(),
            drivers: HashMap::new(),
            listed_drivers: Vec::new(),
            output: None,
            reflection: None,
            _subscriptions: vec![subscription],
        };
        view.relist(cx);
        view
    }

    /// The agent the view is opened on, the CEO when that is `None`.
    pub fn focus(&self) -> Option<AgentId> {
        self.focus
    }

    pub fn listing(&self) -> Listing {
        self.listing
    }

    /// The rows listed now, in order.
    pub fn rows(&self) -> Vec<Entity<AgentRow>> {
        self.listed.iter().map(|a| self.rows[a].clone()).collect()
    }

    pub fn driver_rows(&self) -> Vec<Entity<DriverRow>> {
        self.listed_drivers
            .iter()
            .map(|d| self.drivers[d].clone())
            .collect()
    }

    pub fn output(&self) -> Option<Entity<AgentOutput>> {
        self.output.clone()
    }

    pub fn reflection(&self) -> Option<Entity<Reflection>> {
        self.reflection.clone()
    }

    /// Opens an agent, or the CEO for `None`.
    pub fn open(&mut self, agent: Option<AgentId>, cx: &mut Context<Self>) {
        self.focus = agent;
        self.header = None;
        self.output = None;
        self.reflection = None;
        self.relist(cx);
        cx.notify();
    }

    pub fn set_listing(&mut self, listing: Listing, cx: &mut Context<Self>) {
        self.listing = listing;
        self.relist(cx);
        cx.notify();
    }

    fn state<R>(&self, cx: &Context<Self>, f: impl FnOnce(&ProjectState) -> R) -> Option<R> {
        self.session.read(cx).project(self.project, |p, _| f(p))
    }

    /// Works out what is listed now, making views for anything new. Returns whether that changed.
    fn relist(&mut self, cx: &mut Context<Self>) -> bool {
        let focus = self.focus;
        let Some((agent, children, live)) = self
            .state(cx, |p| {
                let agent = focus.or_else(|| p.project().map(|pr| pr.ceo))?;
                let live: Vec<DriverId> = p.live_drivers().map(|d| d.driver.id).collect();
                // Agents that joined together, such as a team formed at once, are listed by
                // name, so the order does not depend on their ids.
                let mut children = p.children(agent);
                children.sort_by_key(|a| p.agent(*a).map(|r| (r.joined, r.role.clone())));
                Some((agent, children, live))
            })
            .flatten()
        else {
            return false;
        };
        let session = self.session.clone();
        let project = self.project;
        if self
            .header
            .as_ref()
            .is_none_or(|h| h.read(cx).agent() != agent)
        {
            self.header = Some(cx.new(|cx| AgentHeader::new(session.clone(), project, agent, cx)));
        }
        for child in &children {
            if let Some(row) = self.rows.get(child) {
                // Listed again after a while: it may have missed changes.
                if !self.listed.contains(child) {
                    row.update(cx, |r, cx| r.refresh(cx));
                }
            } else {
                let row = cx.new(|cx| AgentRow::new(session.clone(), project, *child, cx));
                self._subscriptions.push(cx.subscribe(
                    &row,
                    |this, _, RowEvent::Open(agent), cx| {
                        this.open(Some(*agent), cx);
                    },
                ));
                self.rows.insert(*child, row);
            }
        }
        for driver in &live {
            if let Some(row) = self.drivers.get(driver) {
                if !self.listed_drivers.contains(driver) {
                    row.update(cx, |r, cx| r.refresh(cx));
                }
            } else {
                let row = cx.new(|cx| DriverRow::new(session.clone(), project, *driver, cx));
                self.drivers.insert(*driver, row);
            }
        }
        // An agent with no agents under it shows what it is working in instead.
        let wants_output = children.is_empty();
        if wants_output
            && self
                .output
                .as_ref()
                .is_none_or(|o| o.read(cx).agent() != agent)
        {
            let reflection = session.clone();
            self.output = Some(cx.new(|cx| AgentOutput::new(session, project, agent, cx)));
            self.reflection = Some(cx.new(|cx| Reflection::new(reflection, project, agent, cx)));
        } else if !wants_output {
            self.output = None;
            self.reflection = None;
        }
        let changed = children != self.listed || live != self.listed_drivers;
        self.listed = children;
        self.listed_drivers = live;
        changed
    }

    fn changed(&mut self, change: &Change, cx: &mut Context<Self>) {
        let ours = match change {
            Change::Project { project, changes } => {
                *project == self.project && changes.parts.contains(&ProjectPart::Team)
            }
            // Harness and machine names, or everything after a new snapshot.
            Change::Reset | Change::Daemon(_) => true,
            Change::Connection { .. } => false,
        };
        if !ours {
            return;
        }
        for agent in &self.listed {
            self.rows[agent].update(cx, |r, cx| r.refresh(cx));
        }
        if let Some(header) = &self.header {
            header.update(cx, |h, cx| h.refresh(cx));
        }
        for driver in &self.listed_drivers {
            self.drivers[driver].update(cx, |r, cx| r.refresh(cx));
        }
        if self.relist(cx) {
            cx.notify();
        }
    }

    fn trail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(focus) = self.focus else {
            return div().into_any_element();
        };
        let trail = self
            .state(cx, |p| {
                p.trail(focus)
                    .into_iter()
                    .filter_map(|a| Some((a, p.agent(a)?.role.clone())))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let last = trail.len().saturating_sub(1);
        let mut crumbs = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(theme::TEXT))
            .text_color(theme::dim());
        for (i, (agent, role)) in trail.into_iter().enumerate() {
            if i > 0 {
                crumbs = crumbs.child("›");
            }
            // Opening the CEO is opening the view's start.
            let target = (i > 0).then_some(agent);
            crumbs = crumbs.child(
                div()
                    .id(("crumb", i))
                    .debug_selector(move || format!("crumb-{i}"))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_color(if i == last {
                        theme::text()
                    } else {
                        theme::muted()
                    })
                    .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
                    .on_click(cx.listener(move |this, _, _, cx| this.open(target, cx)))
                    .child(role),
            );
        }
        crumbs.into_any_element()
    }

    fn switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.listing;
        widgets::segments([Listing::Agents, Listing::Drivers].map(|listing| {
            let label = match listing {
                Listing::Agents => "Agents",
                Listing::Drivers => "Drivers",
            };
            widgets::segment(label, label, on == listing)
                .debug_selector(move || format!("listing-{label}"))
                .on_click(cx.listener(move |this, _, _, cx| this.set_listing(listing, cx)))
                .into_any_element()
        }))
        .into_any_element()
    }

    fn agents(&self, cx: &mut Context<Self>) -> AnyElement {
        let names: Vec<String> = self
            .state(cx, |p| {
                self.listed
                    .iter()
                    .map(|a| p.agent(*a).map_or_else(String::new, |r| r.role.clone()))
                    .collect()
            })
            .unwrap_or_default();
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .when(self.focus.is_none() && self.output.is_none(), |d| {
                d.justify_center()
            })
            .gap(px(18.))
            .children(self.header.clone().map(|h| {
                h.cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(row::HEADER_HEIGHT)),
                )
            }))
            .when(!self.listed.is_empty(), |d| {
                d.child(
                    div()
                        .w_full()
                        .max_w(px(460.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .children(self.listed.iter().zip(names).map(|(agent, name)| {
                            // The wrapper is part of this view, so tests find a row by name even
                            // while the row itself is reused from its cache.
                            div()
                                .w_full()
                                .debug_selector(move || format!("agent-{name}"))
                                .child(self.rows[agent].clone().cached(
                                    StyleRefinement::default().w_full().h(px(row::ROW_HEIGHT)),
                                ))
                        })),
                )
            })
            .children(
                self.output
                    .clone()
                    .map(|o| div().flex_1().min_h_0().w_full().child(o)),
            )
            .children(
                self.reflection
                    .clone()
                    .map(|r| div().flex_none().w_full().child(r)),
            )
            .into_any_element()
    }

    fn drivers(&self) -> AnyElement {
        let head = drivers::COLUMNS.map(|(name, _)| name.to_owned());
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(drivers::line(head, true))
            .children(self.listed_drivers.iter().map(|d| {
                self.drivers[d].clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(drivers::DRIVER_ROW_HEIGHT)),
                )
            }))
            .when(self.listed_drivers.is_empty(), |d| {
                d.child(
                    div()
                        .p(px(24.))
                        .text_size(px(theme::TEXT))
                        .text_color(theme::dim())
                        .child("No driver is live. A driver starts when an agent has work."),
                )
            })
            .into_any_element()
    }
}

impl Render for TeamView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.listing {
            Listing::Agents => self.agents(cx),
            Listing::Drivers => self.drivers(),
        };
        div()
            .id("team")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .py(px(14.))
            .px(px(16.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .font_weight(FontWeight::NORMAL)
                    .child(if self.listing == Listing::Agents {
                        self.trail(cx)
                    } else {
                        div().into_any_element()
                    })
                    .child(self.switch(cx)),
            )
            .child(body)
    }
}
