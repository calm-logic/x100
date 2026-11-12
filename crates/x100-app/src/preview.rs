//! The Preview tab: a link that opens the current development build in the builder's own browser.
//! Showing the app inside the window is a later feature.

use gpui::{Context, Entity, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::ProjectId;

use crate::session::{Session, SessionEvent};
use crate::{theme, widgets};

pub struct PreviewView {
    session: Entity<Session>,
    project: ProjectId,
    address: Option<String>,
    _subscription: Subscription,
}

impl PreviewView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project && changes.parts.contains(&ProjectPart::Preview)
                }
                SessionEvent::Changed(Change::Reset) => true,
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let mut view = Self {
            session,
            project,
            address: None,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// The address of the development build, when the project has one.
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    /// Opens the address in the builder's browser.
    pub fn open_in_browser(&self, cx: &mut Context<Self>) {
        if let Some(address) = &self.address {
            cx.open_url(address);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| {
                p.project().and_then(|pr| pr.preview.clone())
            })
            .flatten();
        if now != self.address {
            self.address = now;
            cx.notify();
        }
    }
}

impl Render for PreviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div().size_full().flex().flex_col().gap(px(10.)).p(px(24.));
        match self.address.clone() {
            Some(address) => page
                .child(
                    div()
                        .text_size(px(theme::TEXT))
                        .text_color(theme::muted())
                        .child("The current development build is at"),
                )
                .child(
                    div()
                        .id("preview-link")
                        .debug_selector(|| "preview-link".into())
                        .text_size(px(15.))
                        .text_color(theme::link())
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(|this, _, _, cx| this.open_in_browser(cx)))
                        .child(address),
                )
                .child(
                    div().mt(px(6.)).child(
                        widgets::button("preview-open", "Open in your browser")
                            .debug_selector(|| "preview-open".into())
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_browser(cx))),
                    ),
                ),
            None => page.child(
                div()
                    .text_size(px(theme::TEXT))
                    .text_color(theme::dim())
                    .child("There is no preview yet. A link appears here once the team has a development build running."),
            ),
        }
    }
}
