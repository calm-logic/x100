//! Changing how an agent looks: a popover anchored to its face on its page, with two tabs, Outline
//! and Color. Each shows the choices as this agent's face, the current one marked. A click applies
//! at once through `agents.set_appearance`, and the face changes everywhere when the state does.
//! Escape or a click outside closes it.

use gpui::{
    Context, DismissEvent, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    Subscription, Window, div, prelude::*, px,
};
use x100_client::Change;
use x100_core::appearance::{Appearance, Hue, Outline};
use x100_core::fold::ProjectPart;
use x100_core::id::{AgentId, ProjectId};

use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{face, theme, widgets, words};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookTab {
    Outline,
    Color,
}

pub struct LookPicker {
    session: gpui::Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    tab: LookTab,
    focus: FocusHandle,
    outcome: Option<Outcome>,
    _subscription: Subscription,
}

impl EventEmitter<DismissEvent> for LookPicker {}

impl Focusable for LookPicker {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LookPicker {
    pub fn new(
        session: gpui::Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&session, move |this, _, event, cx| {
            if let SessionEvent::Changed(Change::Project { project, changes }) = event
                && *project == this.project
                && changes.parts.contains(&ProjectPart::Looks)
            {
                cx.notify();
            }
        });
        Self {
            session,
            project,
            agent,
            tab: LookTab::Outline,
            focus: cx.focus_handle(),
            outcome: None,
            _subscription: subscription,
        }
    }

    pub fn agent(&self) -> AgentId {
        self.agent
    }

    pub fn tab(&self) -> LookTab {
        self.tab
    }

    pub fn show(&mut self, tab: LookTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    fn look(&self, cx: &gpui::App) -> Appearance {
        self.session
            .read(cx)
            .project(self.project, |p, _| {
                p.agent(self.agent).map(|a| a.appearance)
            })
            .flatten()
            .unwrap_or_default()
    }

    /// Sends the new look. The popover stays open, so the builder can try another.
    pub fn apply(&mut self, look: Appearance, cx: &mut Context<Self>) {
        let client = self.session.read(cx).client().clone();
        let (project, agent) = (self.project, self.agent);
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.set_appearance(project, agent, look).await },
            |this, result, cx| {
                this.outcome = result.err();
                cx.notify();
            },
        );
    }

    fn choice(
        &self,
        selector: String,
        look: Appearance,
        on: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(gpui::SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .size(px(42.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(10.))
            .border_1()
            .border_color(if on {
                gpui::rgba(0xffffffa0)
            } else {
                gpui::rgba(0x00000000)
            })
            .when(on, |d| d.bg(theme::selected()))
            .cursor_pointer()
            .hover(|s| s.bg(theme::hover()))
            .on_click(cx.listener(move |this, _, _, cx| this.apply(look, cx)))
            .child(face::face(26., look))
    }
}

impl Render for LookPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.look(cx);
        let choices: Vec<gpui::AnyElement> = match self.tab {
            LookTab::Outline => Outline::ALL
                .into_iter()
                .map(|o| {
                    let look = Appearance { outline: o, ..now };
                    let name = words::outline(o).to_lowercase();
                    self.choice(format!("look-outline-{name}"), look, o == now.outline, cx)
                        .into_any_element()
                })
                .collect(),
            LookTab::Color => std::iter::once(None)
                .chain(Hue::ALL.into_iter().map(Some))
                .map(|h| {
                    let look = Appearance { hue: h, ..now };
                    let name = words::hue(h).to_lowercase().replace(' ', "-");
                    self.choice(format!("look-color-{name}"), look, h == now.hue, cx)
                        .into_any_element()
                })
                .collect(),
        };
        let tabs = [(LookTab::Outline, "Outline"), (LookTab::Color, "Color")];
        let on = tabs.iter().position(|(t, _)| *t == self.tab);
        let toggle = widgets::segments(
            "look-tabs",
            on,
            tabs.map(|(tab, label)| {
                widgets::segment(("look-tab", tab as usize), label, tab == self.tab)
                    .debug_selector(move || format!("look-tab-{}", label.to_lowercase()))
                    .on_click(cx.listener(move |this, _, _, cx| this.show(tab, cx)))
                    .into_any_element()
            }),
        );
        div()
            .id("look-popover")
            .debug_selector(|| "look-popover".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            .occlude()
            .w(px(250.))
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .rounded(px(14.))
            .border_1()
            .border_color(theme::line())
            .bg(theme::raised())
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., 0.6),
                offset: gpui::point(px(0.), px(8.)),
                blur_radius: px(24.),
                spread_radius: px(0.),
                inset: false,
            }])
            .text_color(theme::text())
            .child(toggle)
            .child(div().flex().flex_wrap().gap(px(4.)).children(choices))
            .child(
                div()
                    .debug_selector(|| "look-now".into())
                    .text_size(px(theme::TINY))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::muted())
                    .child(format!(
                        "{} · {}",
                        words::outline(now.outline),
                        words::hue(now.hue)
                    )),
            )
            .children(self.outcome.as_ref().map(notice::line))
    }
}
