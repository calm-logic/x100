//! Colours, type and sizes, taken from the app mock on the marketing page. Dark only for now:
//! theming is a later feature.

use gpui::{Hsla, Rgba, rgb, rgba};
use x100_core::appearance::Hue;

/// The window behind everything.
pub fn bg() -> Rgba {
    rgb(0x0e0e10)
}
/// A panel slightly lifted from the window: code blocks, the box for a first message.
pub fn raised() -> Rgba {
    rgb(0x161618)
}
/// Hairlines between areas and around rows, as `rgba(255,255,255,.14)` on the mock.
pub fn line() -> Rgba {
    rgba(0xffffff24)
}
/// A selected project or tab.
pub fn selected() -> Rgba {
    rgba(0xffffff1a)
}
pub fn hover() -> Rgba {
    rgba(0xffffff0d)
}
pub fn text() -> Rgba {
    rgb(0xffffff)
}
/// Body text in documents and replies.
pub fn body() -> Rgba {
    rgb(0xc9c9ce)
}
pub fn muted() -> Rgba {
    rgb(0x9b9ba1)
}
pub fn dim() -> Rgba {
    rgb(0x6c6c70)
}
/// Live: the sidebar's indicator, a tab's dot.
pub fn live() -> Rgba {
    rgb(0x2fd27a)
}
/// Waiting on the builder.
pub fn ask() -> Rgba {
    rgb(0xd4bf7b)
}
pub fn error() -> Rgba {
    rgb(0xff6b62)
}
/// The builder's own messages.
pub fn mine() -> Rgba {
    rgb(0x2c2c30)
}
/// The CEO's replies.
pub fn theirs() -> Rgba {
    rgb(0x1c1c1e)
}
pub fn meter_track() -> Rgba {
    rgba(0xffffff24)
}
pub fn pip_off() -> Rgba {
    rgba(0xffffff1f)
}
pub fn pip_in() -> Rgba {
    rgba(0xffffff66)
}
pub fn link() -> Rgba {
    rgb(0x58a6ff)
}
pub fn code() -> Rgba {
    rgb(0xffa657)
}
/// An agent's face: its colour, or the text colour without one. Bright enough for a 1.5px line on
/// the window's near-black, and far enough apart to tell eight agents apart.
pub fn hue(h: Option<Hue>) -> Rgba {
    match h {
        None => text(),
        Some(Hue::Red) => rgb(0xff6b6b),
        Some(Hue::Orange) => rgb(0xff9f45),
        Some(Hue::Yellow) => rgb(0xf5d150),
        Some(Hue::Green) => rgb(0x5ad48a),
        Some(Hue::Teal) => rgb(0x3fd0c9),
        Some(Hue::Blue) => rgb(0x5aa9ff),
        Some(Hue::Violet) => rgb(0xa78bfa),
        Some(Hue::Pink) => rgb(0xf472b6),
    }
}
pub fn transparent() -> Hsla {
    gpui::transparent_black()
}

/// Manrope on the mock. Lato is the nearest that a Linux desktop usually has; GPUI falls back to
/// its own default where it is missing.
pub const FONT: &str = if cfg!(target_os = "macos") {
    ".SystemUIFont"
} else {
    "Lato"
};
pub const MONO: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};

pub const SIDEBAR_WIDTH: f32 = 170.;
/// The chat takes 0.8 parts of what the sidebar leaves and the main panel 1.5, as on the mock.
pub const CHAT_SHARE: f32 = 0.8 / 2.3;
pub const TEXT: f32 = 13.;
pub const SMALL: f32 = 12.;
pub const TINY: f32 = 11.;
pub const ROW_TEXT: f32 = 14.;
