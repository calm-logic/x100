//! The window's root: the projects sidebar, the chat with the selected project's CEO, and the
//! main panel. Before a project's first message the main panel is hidden and the chat fills the
//! rest of the window. With no project selected, a new one is being started: the CEO and the box
//! for its first message.

use std::collections::HashMap;

use gpui::{Context, Entity, StyleRefinement, Subscription, Window, div, prelude::*, px, relative};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::ProjectId;

use crate::chat::{self, ChatView, StartView};
use crate::session::{Session, SessionEvent};
use crate::sidebar::Sidebar;
use crate::tabs::MainPanel;
use crate::theme;

/// A project's views, made the first time it is selected and kept while the window is open.
pub struct ProjectViews {
    pub chat: Entity<ChatView>,
    pub main: Entity<MainPanel>,
}

pub struct Shell {
    session: Entity<Session>,
    sidebar: Entity<Sidebar>,
    projects: HashMap<ProjectId, ProjectViews>,
    /// The start of a new project, made the first time it is shown.
    start: Option<Entity<StartView>>,
    /// Whether the selected project had its first message when last drawn.
    started: bool,
    title: String,
    _subscription: Subscription,
}

impl Shell {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(session.clone(), cx));
        let subscription = cx.subscribe(&session, |this, _, event, cx| match event {
            SessionEvent::Selected => cx.notify(),
            SessionEvent::Changed(Change::Project { project, changes })
                if changes.parts.contains(&ProjectPart::Chat)
                    && Some(*project) == this.session.read(cx).selected() =>
            {
                // The first message opens the main panel, and a name changes the title.
                if this.started != this.selected_started(cx) || this.title != this.title_of(cx) {
                    cx.notify();
                }
            }
            SessionEvent::Changed(Change::Reset | Change::Connection { .. }) => cx.notify(),
            _ => {}
        });
        Self {
            session,
            sidebar,
            projects: HashMap::new(),
            start: None,
            started: false,
            title: String::new(),
            _subscription: subscription,
        }
    }

    pub fn session(&self) -> &Entity<Session> {
        &self.session
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    /// The views of a project, once it has been selected.
    pub fn project(&self, project: ProjectId) -> Option<&ProjectViews> {
        self.projects.get(&project)
    }

    /// The start of a new project, once it has been shown.
    pub fn start(&self) -> Option<&Entity<StartView>> {
        self.start.as_ref()
    }

    /// The window's title as last set.
    pub fn title(&self) -> &str {
        &self.title
    }

    fn selected_started(&self, cx: &Context<Self>) -> bool {
        let session = self.session.read(cx);
        session
            .selected()
            .and_then(|p| session.project(p, |p, _| chat::started(p)))
            .unwrap_or(false)
    }

    /// "x100" with the selected project's name, or "Untitled" until its CEO names it.
    fn title_of(&self, cx: &Context<Self>) -> String {
        let session = self.session.read(cx);
        match session.selected() {
            Some(p) => {
                let name = session.project(p, |p, _| crate::words::project_name(Some(p)));
                format!("x100 · {}", name.unwrap_or_default())
            }
            None => "x100".into(),
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let selected = session.selected();
        let connected = session.connected();
        self.started = self.selected_started(cx);
        let title = self.title_of(cx);
        // Told to the platform only when it changes: this runs on every frame.
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }

        let views = selected.map(|project| {
            let session = self.session.clone();
            let views = self
                .projects
                .entry(project)
                .or_insert_with(|| ProjectViews {
                    chat: cx.new(|cx| ChatView::new(session.clone(), project, window, cx)),
                    main: cx.new(|cx| MainPanel::new(session, project, cx)),
                });
            (views.chat.clone(), views.main.clone())
        });

        let body = div().flex_1().min_w_0().h_full().flex();
        let body = match views {
            None => {
                let session = self.session.clone();
                let start = self
                    .start
                    .get_or_insert_with(|| cx.new(|cx| StartView::new(session, window, cx)));
                body.child(start.clone())
            }
            Some((chat, _)) if !self.started => {
                body.child(chat.cached(StyleRefinement::default().size_full()))
            }
            // The main panel is not cached: its rows change often, and a dirty cached view
            // redraws everything under it, row caches included.
            Some((chat, main)) => body
                .child(
                    div()
                        .h_full()
                        .w(relative(theme::CHAT_SHARE))
                        .flex_none()
                        .border_r_1()
                        .border_color(theme::line())
                        .child(chat.cached(StyleRefinement::default().size_full())),
                )
                .child(div().flex_1().min_w_0().h_full().child(main)),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg())
            .text_color(theme::text())
            .font_family(theme::FONT)
            .when(!connected, |d| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(14.))
                        .py(px(6.))
                        .bg(gpui::rgb(0x2a1f10))
                        .text_size(px(theme::SMALL))
                        .text_color(theme::ask())
                        .child("Reconnecting to the daemon…"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(theme::SIDEBAR_WIDTH))
                            .flex_none()
                            .h_full()
                            .border_r_1()
                            .border_color(theme::line())
                            .child(self.sidebar.clone()),
                    )
                    .child(body),
            )
    }
}
