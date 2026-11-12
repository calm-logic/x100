//! The projects sidebar: every project with its live indicator and agent count, a project not set
//! up yet as "Untitled", and "+ New project", which opens the start of another one. That
//! project is created only when its first message is sent.

use gpui::{Context, Entity, FontWeight, SharedString, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::ProjectId;

use crate::session::{Session, SessionEvent};
use crate::{theme, widgets, words};

/// The sidebar's line for one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLine {
    pub id: ProjectId,
    pub name: String,
    pub live: bool,
    pub agents: usize,
}

pub struct Sidebar {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl Sidebar {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { changes, .. }) => {
                    changes.parts.contains(&ProjectPart::Sidebar)
                        || changes.parts.contains(&ProjectPart::Team)
                }
                SessionEvent::Changed(_) | SessionEvent::Selected => true,
                SessionEvent::Preview => false,
            };
            if ours {
                cx.notify();
            }
        });
        Self {
            session,
            _subscription: subscription,
        }
    }

    /// What the sidebar lists, in the daemon's order.
    pub fn lines(&self, cx: &gpui::App) -> Vec<ProjectLine> {
        self.session.read(cx).client().with_state(|s| {
            s.daemon
                .projects()
                .map(|(id, _)| {
                    let p = s.projects.get(&id);
                    ProjectLine {
                        id,
                        name: words::project_name(p),
                        live: p.is_some_and(|p| p.is_live()),
                        agents: p.map_or(0, |p| p.agent_count()),
                    }
                })
                .collect()
        })
    }

    /// Opens the start of a new project.
    pub fn start_new(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |s, cx| s.start_new(cx));
        cx.notify();
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.session.read(cx).selected();
        let lines = self.lines(cx);
        let rows = lines.into_iter().map(|line| {
            let on = Some(line.id) == selected;
            let id = line.id;
            div()
                .id(SharedString::from(id.to_string()))
                .debug_selector({
                    let name = line.name.clone();
                    move || format!("project-{name}")
                })
                .flex()
                .items_center()
                .gap(px(8.))
                .p(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_color(if on { theme::text() } else { theme::muted() })
                .when(on, |d| d.bg(theme::selected()))
                .hover(|s| s.text_color(theme::text()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.session.update(cx, |s, cx| s.select(id, cx))
                }))
                .child(widgets::dot(
                    7.,
                    if line.live {
                        theme::live()
                    } else {
                        gpui::rgb(0x3a3a3d)
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(line.name),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(theme::TINY))
                        .text_color(theme::dim())
                        .child(words::agents(line.agents)),
                )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(10.))
            .py(px(14.))
            .text_size(px(theme::TEXT))
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .text_size(px(theme::TINY))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::dim())
                    .child("PROJECTS"),
            )
            .children(rows)
            .child(
                div()
                    .id("new-project")
                    .debug_selector(|| "new-project".into())
                    .mt(px(6.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_color(theme::dim())
                    .hover(|s| s.text_color(theme::text()).bg(theme::hover()))
                    // Starting a new project is a place of its own, shown as selected.
                    .when(selected.is_none(), |d| {
                        d.text_color(theme::text()).bg(theme::selected())
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.start_new(cx)))
                    .child("+ New project"),
            )
    }
}
