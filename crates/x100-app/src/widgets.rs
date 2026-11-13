//! The small pieces the mock repeats: an agent's orb, the effort pips, a progress meter, a pill, a
//! segmented switch, buttons, and Settings' row of a label and its control.

use gpui::{
    AnyElement, BoxShadow, Div, ElementId, Rgba, SharedString, Stateful, div, hsla, point,
    prelude::*, px, relative,
};
use x100_core::effort::{Effort, EffortRange};

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Orb {
    /// The CEO: white, with dark eyes and a glow.
    Ceo,
    /// Before the first message: the CEO as an outline.
    Outline,
    /// Any other agent: dark, with a white rim.
    Agent,
}

pub fn orb(size: f32, kind: Orb) -> Div {
    let (fill, eyes, rim): (Rgba, Rgba, f32) = match kind {
        Orb::Ceo => (theme::text(), gpui::rgb(0x000000), 1.5),
        Orb::Outline => (gpui::rgba(0x00000000), theme::text(), 2.),
        Orb::Agent => (gpui::rgb(0x0a0a0b), theme::text(), 1.5),
    };
    let eye = |left: bool| {
        div()
            .absolute()
            .top(relative(0.35))
            .when(left, |d| d.left(relative(0.26)))
            .when(!left, |d| d.right(relative(0.26)))
            .w(relative(0.16))
            .h(relative(0.22))
            .rounded_full()
            .bg(eyes)
    };
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .border(px(rim))
        .border_color(theme::text())
        .bg(fill)
        .when(kind == Orb::Ceo, |d| {
            d.shadow(vec![BoxShadow {
                color: hsla(0., 0., 1., 0.45),
                offset: point(px(0.), px(0.)),
                blur_radius: px(size / 4.),
                spread_radius: px(0.),
                inset: false,
            }])
        })
        .child(eye(true))
        .child(eye(false))
}

/// The four effort levels: lit inside `range`, bright at `now`.
pub fn pips(range: EffortRange, now: Option<Effort>) -> Div {
    div().flex().gap(px(3.)).children(Effort::ALL.map(|e| {
        let colour = if Some(e) == now {
            theme::text()
        } else if range.contains(e) {
            theme::pip_in()
        } else {
            theme::pip_off()
        };
        div().w(px(14.)).h(px(3.)).rounded(px(2.)).bg(colour)
    }))
}

/// A thin progress bar, `done` out of 100.
pub fn meter(done: u8) -> Div {
    div()
        .w_full()
        .h(px(3.))
        .rounded(px(2.))
        .bg(theme::meter_track())
        .child(
            div()
                .h_full()
                .w(relative(f32::from(done.min(100)) / 100.))
                .rounded(px(2.))
                .bg(theme::text()),
        )
}

/// A rounded label with a hairline rim, such as the driver an agent is on.
pub fn pill(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .border_1()
        .border_color(theme::line())
        .text_size(px(theme::TINY))
        .text_color(theme::body())
        .whitespace_nowrap()
        .child(text.into())
}

/// A small dot: live, unseen or idle.
pub fn dot(size: f32, colour: Rgba) -> Div {
    div().flex_none().size(px(size)).rounded_full().bg(colour)
}

/// A button in a segmented switch, rimmed when on.
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(10.))
        .py(px(4.))
        .rounded_full()
        .text_size(px(theme::SMALL))
        .whitespace_nowrap()
        .cursor_pointer()
        .text_color(if on { theme::text() } else { theme::muted() })
        .when(on, |d| d.border_1().border_color(gpui::rgba(0xffffff99)))
        .when(!on, |d| d.border_1().border_color(gpui::rgba(0x00000000)))
        .hover(|s| s.text_color(theme::text()))
        .child(label.into())
}

/// The segments' frame.
pub fn segments(children: impl IntoIterator<Item = AnyElement>) -> Div {
    div()
        .flex()
        .p(px(3.))
        .gap(px(2.))
        .rounded_full()
        .border_1()
        .border_color(theme::line())
        .children(children)
}

/// A plain button: rimmed text.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(13.))
        .py(px(6.))
        .rounded_full()
        .border_1()
        .border_color(gpui::rgba(0xffffff99))
        .text_size(px(theme::SMALL))
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .child(label.into())
}

/// A round button holding one symbol, such as "+".
pub fn round_button(id: impl Into<ElementId>, symbol: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .rounded_full()
        .border_1()
        .border_color(gpui::rgba(0xffffff99))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(16.))
        .line_height(px(16.))
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|s| s.bg(theme::hover()))
        .child(symbol)
}

/// A label on the left and its control on the right, under a hairline.
pub fn row(
    label: impl Into<SharedString>,
    detail: Option<String>,
    control: impl IntoElement,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .py(px(12.))
        .border_b_1()
        .border_color(theme::line())
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(theme::TEXT))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label.into()),
                )
                .children(detail.map(|d| {
                    div()
                        .text_size(px(theme::TINY))
                        .text_color(theme::muted())
                        .child(d)
                })),
        )
        .child(control)
}
