//! The small pieces the mock repeats: the effort pips, a progress meter, a pill, a
//! segmented switch, buttons, and Settings' row of a label and its control.

use gpui::{Div, ElementId, Rgba, SharedString, Stateful, div, prelude::*, px, relative};
use x100_core::effort::{Effort, EffortRange};

use crate::theme;

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

pub use crate::glass::segments;

/// An option in [`segments`]. The chosen one is bright, and sits on the toggle's glass.
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
        .hover(|s| s.text_color(theme::text()))
        .child(label.into())
}

/// A secondary button: rimmed text. A primary action is [`crate::glass::primary`].
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
