//! The box the builder writes in, for every message to the CEO. It carries attachments, and
//! answers the CEO's open question unless told not to. For a new project it also creates the
//! project as the first message is sent: files chosen before then are uploaded once it exists.

use std::path::PathBuf;

use gpui::{
    AnyElement, BoxShadow, Context, Entity, ExternalPaths, Focusable, PathPromptOptions,
    Subscription, Window, div, hsla, point, prelude::*, px,
};
use x100_core::file::FileRef;
use x100_core::id::{MessageId, ProjectId};

use super::attach;
use crate::input::{InputEvent, TextInput};
use crate::session::Session;
use crate::{theme, widgets};

/// Which question a message answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// The CEO's latest open question, if there is one.
    Latest,
    /// None: the builder dismissed the question for this message.
    Nothing,
    To(MessageId),
}

/// Where a message goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Project(ProjectId),
    /// A project that does not exist yet, on an existing folder if one is named.
    New {
        folder: Option<String>,
    },
}

pub struct Composer {
    session: Entity<Session>,
    target: Target,
    input: Entity<TextInput>,
    attachments: Vec<FileRef>,
    /// Files chosen for a new project, uploaded once the project exists.
    pending: Vec<PathBuf>,
    uploading: usize,
    reply: Reply,
    sending: bool,
    error: Option<String>,
    /// A field for a file's path, for desktops without a file chooser (WSL has none).
    by_path: Option<(Entity<TextInput>, Subscription)>,
    _subscription: Subscription,
}

impl Composer {
    pub fn new(session: Entity<Session>, target: Target, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Write to the CEO", true, cx));
        let subscription = cx.subscribe(&input, |this, _, InputEvent::Submit, cx| this.send(cx));
        Self {
            session,
            target,
            input,
            attachments: Vec::new(),
            pending: Vec::new(),
            uploading: 0,
            reply: Reply::Latest,
            sending: false,
            error: None,
            by_path: None,
            _subscription: subscription,
        }
    }

    pub fn attachments(&self) -> &[FileRef] {
        &self.attachments
    }

    /// The files chosen for a new project, not uploaded yet.
    pub fn pending(&self) -> &[PathBuf] {
        &self.pending
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The existing folder a new project starts on, or none.
    pub fn set_folder(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        if let Target::New { folder: f } = &mut self.target {
            *f = folder;
            cx.notify();
        }
    }

    fn project(&self) -> Option<ProjectId> {
        match self.target {
            Target::Project(p) => Some(p),
            Target::New { .. } => None,
        }
    }

    /// Shows or hides the field for a file's path.
    fn toggle_by_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.by_path.take().is_some() {
            cx.notify();
            return;
        }
        let field = cx.new(|cx| TextInput::new("Path to a file, then Enter", false, cx));
        let submit = cx.subscribe(&field, |this, field, InputEvent::Submit, cx| {
            let path = crate::platform::expand_home(field.read(cx).text().trim());
            if !path.is_empty() {
                this.by_path = None;
                this.attach(vec![PathBuf::from(path)], cx);
            }
        });
        window.focus(&field.focus_handle(cx), cx);
        self.by_path = Some((field, submit));
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input.focus_handle(cx), cx);
    }

    /// Answer this question with the next message.
    pub fn answer(&mut self, question: MessageId, cx: &mut Context<Self>) {
        self.reply = Reply::To(question);
        cx.notify();
    }

    /// The question the next message answers, with its text.
    fn answering(&self, cx: &Context<Self>) -> Option<(MessageId, String)> {
        let reply = self.reply;
        self.session
            .read(cx)
            .project(self.project()?, |p, _| {
                let mut open: Vec<_> = p.open_questions().collect();
                open.sort_by_key(|m| m.at);
                let question = match reply {
                    Reply::Nothing => None,
                    Reply::Latest => open.last(),
                    Reply::To(id) => open.iter().find(|m| m.message.id == id),
                }?;
                Some((question.message.id, question.message.body.clone()))
            })
            .flatten()
    }

    /// Opens the platform's file chooser.
    pub fn choose_files(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            let paths = match chosen.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Err(e)) => {
                    let _ = this.update(cx, |c, cx| {
                        c.error = Some(format!("Could not open a file chooser: {e}"));
                        cx.notify();
                    });
                    return;
                }
                _ => return,
            };
            let _ = this.update(cx, |c, cx| c.attach(paths, cx));
        })
        .detach();
    }

    /// Uploads each file and attaches it once the daemon has it, one after another, so they are
    /// attached in the order they were chosen however long each takes.
    pub fn attach(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(project) = self.project() else {
            self.pending.extend(paths);
            cx.notify();
            return;
        };
        self.uploading += paths.len();
        let client = self.session.read(cx).client().clone();
        cx.spawn(async move |this, cx| {
            for path in paths {
                let read = cx.background_spawn({
                    let path = path.clone();
                    async move { std::fs::read(path) }
                });
                let uploaded = attach::upload(client.clone(), project, path, read.await).await;
                let alive = this.update(cx, |c, cx| {
                    c.uploading -= 1;
                    match uploaded {
                        Ok(up) => {
                            c.session.update(cx, |s, _| {
                                s.keep_image(up.file.id, &up.file.media_type, &up.bytes)
                            });
                            c.attachments.push(up.file);
                        }
                        Err(e) => c.error = Some(e),
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub fn send(&mut self, cx: &mut Context<Self>) {
        let body = self.input.read(cx).text().trim().to_owned();
        let empty = self.attachments.is_empty() && self.pending.is_empty();
        if self.sending || (body.is_empty() && empty) {
            return;
        }
        if self.uploading > 0 {
            self.error = Some("Wait for the files to finish uploading.".into());
            cx.notify();
            return;
        }
        let client = self.session.read(cx).client().clone();
        let mut files: Vec<_> = self.attachments.iter().map(|f| f.id).collect();
        let reply = self.answering(cx).map(|(id, _)| id);
        let target = self.target.clone();
        let pending = std::mem::take(&mut self.pending);
        self.sending = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // A new project is made now, and what was chosen for it uploaded to it.
            let project = match target {
                Target::Project(p) => p,
                Target::New { folder } => match client.create_project(folder).await {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = this.update(cx, |c, cx| c.failed(None, pending, e.to_string(), cx));
                        return;
                    }
                },
            };
            for (i, path) in pending.iter().enumerate() {
                let read = cx.background_spawn({
                    let path = path.clone();
                    async move { std::fs::read(path) }
                });
                match attach::upload(client.clone(), project, path.clone(), read.await).await {
                    Ok(up) => files.push(up.file.id),
                    Err(e) => {
                        let rest = pending[i..].to_vec();
                        let _ = this.update(cx, |c, cx| c.failed(Some(project), rest, e, cx));
                        return;
                    }
                }
            }
            let sent = client.send_chat(project, body, files, reply).await;
            let _ = this.update(cx, |c, cx| match sent {
                Ok(_) => {
                    c.sending = false;
                    c.input.update(cx, |i, cx| i.set_text("", cx));
                    c.attachments.clear();
                    c.reply = Reply::Latest;
                    if c.project().is_none() {
                        // The start is ready for another project; this one is shown.
                        c.target = Target::New { folder: None };
                        c.session.update(cx, |s, cx| s.select(project, cx));
                    }
                    cx.notify();
                }
                Err(e) => c.failed(Some(project), Vec::new(), e.to_string(), cx),
            });
        })
        .detach();
    }

    /// A send that did not go through. A project made on the way is kept, so trying again sends
    /// to it rather than making another.
    fn failed(
        &mut self,
        made: Option<ProjectId>,
        pending: Vec<PathBuf>,
        error: String,
        cx: &mut Context<Self>,
    ) {
        self.sending = false;
        if let Some(project) = made {
            self.target = Target::Project(project);
        }
        self.pending.extend(pending);
        self.error = Some(format!("Not sent: {error}"));
        cx.notify();
    }

    fn render_attachments(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut previews: Vec<_> = match self.project() {
            Some(project) => self
                .attachments
                .iter()
                .map(|f| {
                    let image = self
                        .session
                        .update(cx, |s, cx| s.image(project, f.id, &f.media_type, cx));
                    attach::preview(f, image)
                })
                .collect(),
            None => Vec::new(),
        };
        previews.extend(self.pending.iter().map(|p| attach::pending(p)));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .children(previews)
            .when(self.uploading > 0, |d| {
                d.child(
                    div()
                        .text_size(px(theme::TINY))
                        .text_color(theme::muted())
                        .child("Uploading…"),
                )
            })
            .into_any_element()
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Before a project's first message the box draws the eye.
        let first = match self.project() {
            Some(p) => !self
                .session
                .read(cx)
                .project(p, |p, _| super::started(p))
                .unwrap_or(true),
            None => true,
        };
        let answering = self.answering(cx);
        let attachments = self.render_attachments(cx);
        div()
            .id("composer")
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(12.))
            .border_1()
            .border_color(gpui::rgba(0xffffff60))
            // Opaque, so the first message's glow stays outside it.
            .bg(gpui::rgb(0x18181a))
            .when(first, |d| {
                // As on the mock.
                d.border_color(gpui::rgba(0xffffffbf))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 1., 0.25),
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(28.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.attach(paths.paths().to_vec(), cx)
            }))
            .when_some(answering, |d, (_, question)| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(theme::TINY))
                        .text_color(theme::ask())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(format!("Answering: {question}")),
                        )
                        .child(
                            div()
                                .id("no-answer")
                                .cursor_pointer()
                                .text_color(theme::muted())
                                .hover(|s| s.text_color(theme::text()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reply = Reply::Nothing;
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                )
            })
            .child(
                div()
                    .id("composer-input")
                    .debug_selector(|| "composer-input".into())
                    .when(first, |d| d.min_h(px(72.)))
                    .cursor_text()
                    // The box is taller than its first line: a click anywhere in it writes there.
                    .on_click(cx.listener(|this, _, window, cx| this.focus(window, cx)))
                    .child(self.input.clone()),
            )
            .child(attachments)
            .when_some(self.by_path.as_ref(), |d, (field, _)| {
                d.child(
                    div()
                        .debug_selector(|| "attach-path".into())
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::line())
                        .child(field.clone()),
                )
            })
            .when_some(self.error.clone(), |d, e| {
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
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(
                                widgets::button("attach", "Attach")
                                    .debug_selector(|| "attach".into())
                                    .on_click(cx.listener(|this, _, _, cx| this.choose_files(cx))),
                            )
                            .child(
                                widgets::button("by-path", "By path")
                                    .debug_selector(|| "attach-by-path".into())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_by_path(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        widgets::button("send", if self.sending { "Sending…" } else { "Send" })
                            .debug_selector(|| "send".into())
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    ),
            )
    }
}
