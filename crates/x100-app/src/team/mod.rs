//! The Team tab. Its top level is the CEO, large, over the leads' rows. Opening an agent, or the
//! CEO's own output, is its drilldown: the agent's head, small, and its name in the top row's
//! left corner with the trail back to the CEO, and everything below that row its live output. An
//! agent that runs agents is opened on their rows, with a switch in the top row to its own output,
//! since it works in a terminal of its own too. A CEO with no leads has nothing else to show, so
//! the top level is its drilldown. Another switch lists the project's live drivers instead.
//!
//! The view itself is not cached and redraws cheaply whenever anything under it does. Each agent
//! and each driver is a cached view of its own. When an event changes the Team part (core's
//! `Changes`), each reads what it shows again and redraws only if that differs, so one agent's
//! progress never redraws another's row.

mod drivers;
mod log;
mod look;
mod output;
mod row;

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Entity, Focusable, FontWeight, StyleRefinement, Subscription, Window, div,
    prelude::*, px,
};
use x100_client::Change;
use x100_core::fold::{ProjectPart, ProjectState};
use x100_core::id::{AgentId, DriverId, ProjectId};

pub use drivers::{DriverModel, DriverRow};
pub use look::{LookPicker, LookTab};
pub use output::{AgentOutput, SegmentShown};
pub use row::{AgentHeader, AgentModel, AgentRow, RowEvent};

use crate::reflection::Reflection;

/// Whether the daemon serves an agent's memories, skills and trials. It does not until M13 and
/// M14 build them, and until then the agent view leaves them out rather than say so three times.
const REFLECTION_SERVED: bool = false;
use crate::session::{Session, SessionEvent};
use crate::{face, theme, widgets, words};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    Agents,
    Drivers,
}

/// What an agent that runs agents is opened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The agents it runs.
    Agents,
    /// What it is working in itself.
    Output,
}

pub struct TeamView {
    session: Entity<Session>,
    project: ProjectId,
    listing: Listing,
    /// The agent the view is opened on; `None` is the CEO.
    focus: Option<AgentId>,
    /// Whether that agent's agents or its output are shown, while it runs any.
    shown: Shown,
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
    /// The popover that changes the opened agent's look, while it is open.
    picker: Option<(Entity<LookPicker>, Subscription)>,
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
            shown: Shown::Agents,
            header: None,
            rows: HashMap::new(),
            listed: Vec::new(),
            drivers: HashMap::new(),
            listed_drivers: Vec::new(),
            output: None,
            reflection: None,
            picker: None,
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

    pub fn shown(&self) -> Shown {
        self.shown
    }

    /// The agent the view is opened on, as its header shows it.
    pub fn header(&self) -> Option<Entity<AgentHeader>> {
        self.header.clone()
    }

    /// Whether the view is an agent's drilldown, its head small in the top row: any agent opened,
    /// or the CEO's own output. Otherwise it is the top level, the CEO large over its leads.
    pub fn drilled(&self) -> bool {
        self.focus.is_some() || self.output.is_some()
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

    pub fn picker(&self) -> Option<Entity<LookPicker>> {
        self.picker.as_ref().map(|(p, _)| p.clone())
    }

    /// Opens the popover that changes the opened agent's look, anchored to its face, and gives it
    /// the keyboard so Escape closes it. A retired agent's look no longer changes.
    pub fn pick_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(header) = &self.header else { return };
        let (agent, retired) = {
            let h = header.read(cx);
            (h.agent(), h.shown().is_none_or(|m| m.retired))
        };
        if retired {
            return;
        }
        let picker = cx.new(|cx| LookPicker::new(self.session.clone(), self.project, agent, cx));
        let closed = cx.subscribe(&picker, |this, _, _: &gpui::DismissEvent, cx| {
            this.picker = None;
            cx.notify();
        });
        window.focus(&picker.focus_handle(cx), cx);
        self.picker = Some((picker, closed));
        cx.notify();
    }

    /// Opens an agent, or the CEO for `None`.
    pub fn open(&mut self, agent: Option<AgentId>, cx: &mut Context<Self>) {
        self.picker = None;
        self.focus = agent;
        self.shown = Shown::Agents;
        self.header = None;
        self.output = None;
        self.reflection = None;
        self.relist(cx);
        cx.notify();
    }

    /// Shows the opened agent's agents or its own output. An agent with no agents shows its
    /// output whatever this says.
    pub fn show(&mut self, shown: Shown, cx: &mut Context<Self>) {
        self.shown = shown;
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
            let header = cx.new(|cx| AgentHeader::new(session.clone(), project, agent, cx));
            // At the top level, clicking the CEO opens its page, as clicking a lead's row does.
            self._subscriptions.push(
                cx.subscribe(&header, |this, _, RowEvent::Open(agent), cx| {
                    this.open(Some(*agent), cx)
                }),
            );
            self.header = Some(header);
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
        let wants_output = children.is_empty() || self.shown == Shown::Output;
        if wants_output
            && self
                .output
                .as_ref()
                .is_none_or(|o| o.read(cx).agent() != agent)
        {
            let reflection = session.clone();
            self.output = Some(cx.new(|cx| AgentOutput::new(session, project, agent, cx)));
            // Memories, skills and trials are mounted once their milestones build them.
            self.reflection = REFLECTION_SERVED
                .then(|| cx.new(|cx| Reflection::new(reflection, project, agent, cx)));
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

    /// The drilldown's corner: the agent's head, small, its name beside it, and the agents above
    /// it back to the CEO, each of which opens it.
    fn corner(&self, cx: &mut Context<Self>) -> AnyElement {
        let ceo = self.state(cx, |p| p.project().map(|pr| pr.ceo)).flatten();
        let Some(m) = self
            .header
            .as_ref()
            .and_then(|h| h.read(cx).shown().cloned())
        else {
            return div().into_any_element();
        };
        let above = self
            .focus
            .and_then(|focus| {
                self.state(cx, |p| {
                    let mut trail = p.trail(focus);
                    trail.pop();
                    trail
                        .into_iter()
                        .filter_map(|a| {
                            let r = p.agent(a)?;
                            Some((Some(a), r.role.clone(), Some(r.appearance)))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        // The CEO's own page has nobody above it; its way back is to the team.
        let above = if self.focus.is_some() && self.focus == ceo {
            vec![(None, "Team".to_owned(), None)]
        } else {
            above
        };
        let picker = self.picker.as_ref().map(|(p, _)| p.clone());
        let head = div()
            .id("drill-head")
            .relative()
            .rounded_full()
            .when(!m.retired, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| this.pick_look(window, cx)))
            })
            .debug_selector(|| "drill-head".into())
            .child(face::face(22., m.look))
            .children(picker.map(|p| {
                div()
                    .absolute()
                    .top(px(28.))
                    .left(px(-4.))
                    .child(gpui::deferred(
                        gpui::anchored().snap_to_window_with_margin(px(8.)).child(p),
                    ))
            }));
        let mut corner = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(head)
            .child(
                div()
                    .debug_selector(|| "drill-name".into())
                    .text_size(px(theme::ROW_TEXT))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .whitespace_nowrap()
                    .child(m.name.clone()),
            );
        if !above.is_empty() {
            let mut crumbs = div()
                .ml(px(4.))
                .pl(px(10.))
                .border_l_1()
                .border_color(theme::line())
                .flex()
                .items_center()
                .gap(px(2.))
                .text_size(px(theme::SMALL))
                .text_color(theme::dim());
            for (i, (agent, role, look)) in above.into_iter().enumerate() {
                if i > 0 {
                    crumbs = crumbs.child("›");
                }
                // Opening the CEO is opening the view's start.
                let target = agent.filter(|_| i > 0);
                crumbs = crumbs.child(
                    div()
                        .id(("crumb", i))
                        .debug_selector(move || format!("crumb-{i}"))
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .text_color(theme::muted())
                        .hover(|s| s.bg(theme::hover()).text_color(theme::text()))
                        .on_click(cx.listener(move |this, _, _, cx| this.open(target, cx)))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .children(look.map(|l| face::face(14., l)))
                        .child(role),
                );
            }
            corner = corner.child(crumbs);
        }
        corner.into_any_element()
    }

    fn switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.listing;
        let listings = [Listing::Agents, Listing::Drivers];
        let at = listings.iter().position(|l| *l == on);
        widgets::segments(
            "listing",
            at,
            listings.map(|listing| {
                let label = match listing {
                    Listing::Agents => "Agents",
                    Listing::Drivers => "Drivers",
                };
                widgets::segment(label, label, on == listing)
                    .debug_selector(move || format!("listing-{label}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_listing(listing, cx)))
                    .into_any_element()
            }),
        )
        .into_any_element()
    }

    /// Between an agent's agents and its own output, when it runs agents.
    fn shown_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.shown;
        let agents = words::agents(self.listed.len());
        widgets::segments(
            "shown",
            Some(on as usize),
            [
                (Shown::Agents, agents),
                (Shown::Output, "Its output".into()),
            ]
            .map(|(shown, label)| {
                let name = match shown {
                    Shown::Agents => "Agents",
                    Shown::Output => "Output",
                };
                // Its own id, since the listing switch beside it has an "Agents" too.
                widgets::segment(("shown", shown as usize), label, on == shown)
                    .debug_selector(move || format!("shown-{name}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.show(shown, cx)))
                    .into_any_element()
            }),
        )
        .into_any_element()
    }

    /// The leads' rows under the CEO, large: the top level when the CEO runs agents.
    fn top_level(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(18.))
            .px(px(16.))
            .py(px(14.))
            .children(self.header.clone().map(|h| {
                h.cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(row::HEADER_HEIGHT)),
                )
            }))
            .child(self.shown_switch(cx))
            .child(self.rows_list(cx))
            .into_any_element()
    }

    /// The rows of the agents listed now, each opening its agent.
    fn rows_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let names: Vec<String> = self
            .state(cx, |p| {
                self.listed
                    .iter()
                    .map(|a| p.agent(*a).map_or_else(String::new, |r| r.role.clone()))
                    .collect()
            })
            .unwrap_or_default();
        div()
            .w_full()
            .max_w(px(460.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(self.listed.iter().zip(names).map(|(agent, name)| {
                // The wrapper is part of this view, so tests find a row by name even while the
                // row itself is reused from its cache.
                div()
                    .w_full()
                    .debug_selector(move || format!("agent-{name}"))
                    .child(
                        self.rows[agent]
                            .clone()
                            .cached(StyleRefinement::default().w_full().h(px(row::ROW_HEIGHT))),
                    )
            }))
            .into_any_element()
    }

    /// Under a drilldown's top row: its live output, edge to edge, or the rows of its agents.
    fn drilldown(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.output {
            Some(output) => div()
                .size_full()
                .flex()
                .flex_col()
                .debug_selector(|| "drill-output".into())
                .child(div().flex_1().min_h_0().w_full().child(output.clone()))
                .children(
                    self.reflection
                        .clone()
                        .map(|r| div().flex_none().w_full().child(r)),
                )
                .into_any_element(),
            None => div()
                .id("drill-agents")
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .px(px(16.))
                .py(px(18.))
                .overflow_y_scroll()
                .child(self.rows_list(cx))
                .into_any_element(),
        }
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
        let agents = self.listing == Listing::Agents;
        let drilled = agents && self.drilled();
        let body = match self.listing {
            Listing::Agents if drilled => self.drilldown(cx),
            Listing::Agents => self.top_level(cx),
            Listing::Drivers => div()
                .id("drivers")
                .size_full()
                .px(px(16.))
                .py(px(14.))
                .overflow_y_scroll()
                .child(self.drivers())
                .into_any_element(),
        };
        let left = if drilled {
            self.corner(cx)
        } else {
            div().into_any_element()
        };
        div()
            .id("team")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(10.))
                    .when(drilled, |d| d.border_b_1().border_color(theme::line()))
                    .font_weight(FontWeight::NORMAL)
                    .child(left)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(drilled && !self.listed.is_empty(), |d| {
                                d.child(self.shown_switch(cx))
                            })
                            .child(self.switch(cx)),
                    ),
            )
            .child(div().flex_1().min_h_0().w_full().child(body))
    }
}
