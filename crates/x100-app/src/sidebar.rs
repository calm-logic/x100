//! The projects sidebar: every project with its live indicator and agent count, and a form to
//! create one from a name and a directory.

use gpui::{
    Context, Entity, Focusable, FontWeight, SharedString, Subscription, Window, div, prelude::*, px,
};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::ProjectId;

use crate::input::{InputEvent, TextInput};
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

struct NewProject {
    name: Entity<TextInput>,
    path: Entity<TextInput>,
    busy: bool,
    error: Option<String>,
    _submit: [Subscription; 2],
}

pub struct Sidebar {
    session: Entity<Session>,
    creating: Option<NewProject>,
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
            creating: None,
            _subscription: subscription,
        }
    }

    /// What the sidebar lists, in the daemon's order.
    pub fn lines(&self, cx: &gpui::App) -> Vec<ProjectLine> {
        self.session.read(cx).client().with_state(|s| {
            s.daemon
                .projects()
                .map(|(id, entry)| {
                    let p = s.projects.get(&id);
                    ProjectLine {
                        id,
                        name: entry.name.clone(),
                        live: p.is_some_and(|p| p.is_live()),
                        agents: p.map_or(0, |p| p.agent_count()),
                    }
                })
                .collect()
        })
    }

    pub fn start_creating(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| TextInput::new("Name", false, cx));
        let path = cx.new(|cx| TextInput::new("Directory", false, cx));
        window.focus(&name.read(cx).focus_handle(cx), cx);
        let submit = [&name, &path]
            .map(|input| cx.subscribe(input, |this, _, InputEvent::Submit, cx| this.create(cx)));
        self.creating = Some(NewProject {
            name,
            path,
            busy: false,
            error: None,
            _submit: submit,
        });
        cx.notify();
    }

    pub fn create(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.creating else {
            return;
        };
        let name = form.name.read(cx).text().trim().to_owned();
        let path = crate::platform::expand_home(form.path.read(cx).text().trim());
        if path.is_empty() {
            form.error = Some("A directory is needed.".into());
            cx.notify();
            return;
        }
        form.busy = true;
        form.error = None;
        let client = self.session.read(cx).client().clone();
        cx.spawn(async move |this, cx| {
            let created = client
                .create_project(path, (!name.is_empty()).then_some(name))
                .await;
            let _ = this.update(cx, |this, cx| {
                match created {
                    Ok(project) => {
                        this.creating = None;
                        this.session.update(cx, |s, cx| s.select(project, cx));
                    }
                    Err(e) => {
                        if let Some(form) = &mut this.creating {
                            form.busy = false;
                            form.error = Some(e.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
        let form = self.creating.as_ref().map(|form| {
            let field = |input: &Entity<TextInput>, name: &'static str| {
                div()
                    .debug_selector(move || name.into())
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(theme::line())
                    .child(input.clone())
            };
            div()
                .mt(px(8.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(field(&form.name, "new-project-name"))
                .child(field(&form.path, "new-project-path"))
                .when_some(form.error.clone(), |d, e| {
                    d.child(
                        div()
                            .text_size(px(theme::TINY))
                            .text_color(theme::error())
                            .child(e),
                    )
                })
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            widgets::button(
                                "create",
                                if form.busy { "Creating…" } else { "Create" },
                            )
                            .debug_selector(|| "create-project".into())
                            .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                        )
                        .child(widgets::button("cancel", "Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.creating = None;
                                cx.notify();
                            },
                        ))),
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
            .children(form)
            .when(self.creating.is_none(), |d| {
                d.child(
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
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_creating(window, cx)),
                        )
                        .child("+ New project"),
                )
            })
    }
}
