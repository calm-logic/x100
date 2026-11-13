//! The start of a new project: the box for the first message, with no form. On the first run it
//! opens with [`super::opening`]. Sending
//! creates the project. A small control under the box names an existing folder for the project
//! to start on instead; a path field is enough, since WSL has no file chooser.

use std::time::Instant;

use gpui::{Context, Entity, Focusable, MouseButton, Subscription, Window, div, prelude::*, px};

use super::{Composer, Target, first_run, opening};
use crate::input::{InputEvent, TextInput};
use crate::session::Session;
use crate::theme;

pub struct StartView {
    composer: Entity<Composer>,
    /// The field for a folder's path, while it is open.
    folder: Option<(Entity<TextInput>, Subscription)>,
    opening: Opening,
}

/// Whether the opening plays, and from when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// Not on this start: a project exists, the OS asks for reduced motion, or it has played.
    Off,
    /// To play from the first frame drawn.
    Due,
    Playing(Instant),
}

impl StartView {
    /// `first_run` is that no project exists yet: the opening plays then, unless the OS asks for
    /// reduced motion.
    pub fn new(
        session: Entity<Session>,
        first_run: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let target = Target::New { folder: None };
        let composer = cx.new(|cx| Composer::new(session, target, cx));
        composer.update(cx, |c, cx| c.focus(window, cx));
        Self {
            composer,
            folder: None,
            opening: if first_run && !cx.reduce_motion() {
                Opening::Due
            } else {
                Opening::Off
            },
        }
    }

    pub fn opening(&self) -> Opening {
        self.opening
    }

    /// Ends the opening where it is: the box is already there under it.
    pub fn skip(&mut self, cx: &mut Context<Self>) {
        if self.opening != Opening::Off {
            self.opening = Opening::Off;
            cx.notify();
        }
    }

    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The folder the project will start on, if one is named.
    fn chosen(&self, cx: &Context<Self>) -> Option<String> {
        match self.composer.read(cx).target() {
            Target::New { folder } => folder.clone(),
            Target::Project(_) => None,
        }
    }

    fn open_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| TextInput::new("Path to a folder, then Enter", false, cx));
        let submit = cx.subscribe_in(
            &field,
            window,
            |this, field, InputEvent::Submit, window, cx| {
                let path = crate::platform::expand_home(field.read(cx).text().trim());
                if !path.is_empty() {
                    this.folder = None;
                    // Back to the box, for the first message.
                    this.composer.update(cx, |c, cx| {
                        c.set_folder(Some(path), cx);
                        c.focus(window, cx);
                    });
                }
            },
        );
        window.focus(&field.focus_handle(cx), cx);
        self.folder = Some((field, submit));
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.set_folder(None, cx));
        cx.notify();
    }

    fn folder_control(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let line = div()
            .mt(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(theme::TINY))
            .text_color(theme::dim());
        if let Some((field, _)) = &self.folder {
            return line
                .child(
                    div()
                        .debug_selector(|| "folder-path".into())
                        .w(px(360.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme::line())
                        .text_size(px(theme::SMALL))
                        .child(field.clone()),
                )
                .into_any_element();
        }
        match self.chosen(cx) {
            Some(folder) => line
                .child(format!("On the existing folder {folder}"))
                .child(
                    div()
                        .id("no-folder")
                        .debug_selector(|| "no-folder".into())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::text()))
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
                        .child("×"),
                )
                .into_any_element(),
            None => line
                .child(
                    div()
                        .id("open-folder")
                        .debug_selector(|| "open-folder".into())
                        .cursor_pointer()
                        .hover(|s| s.text_color(theme::text()))
                        .on_click(cx.listener(|this, _, window, cx| this.open_field(window, cx)))
                        .child("Or start on an existing folder"),
                )
                .into_any_element(),
        }
    }
}

impl Render for StartView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let below = self.folder_control(cx);
        let now = Instant::now();
        let over = match self.opening {
            Opening::Off => None,
            Opening::Due => {
                self.opening = Opening::Playing(now);
                // Ends it once its last frame is drawn, so nothing is laid over the box after.
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(opening::LENGTH).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.opening == Opening::Playing(now) {
                            this.opening = Opening::Off;
                            cx.notify();
                        }
                    });
                })
                .detach();
                Some(now)
            }
            Opening::Playing(at) if now - at < opening::LENGTH => Some(at),
            Opening::Playing(_) => {
                self.opening = Opening::Off;
                None
            }
        };
        div()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.skip(cx)),
            )
            .capture_key_down(cx.listener(|this, _, _, cx| this.skip(cx)))
            .child(first_run(
                self.composer.clone().into_any_element(),
                Some(below),
                over.map(|at| opening::overlay(at).into_any_element()),
            ))
    }
}
