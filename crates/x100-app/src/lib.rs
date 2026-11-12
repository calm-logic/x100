//! The desktop interface, in GPUI, on `x100-client`.
//!
//! The app draws and takes input, and nothing else: everything shown is read from the client's
//! folded state, and every action is a protocol request. Each area is a view over a slice of that
//! state in a module of its own:
//!
//! - [`shell`]: the window's root and its three areas.
//! - [`sidebar`]: the projects, and creating one.
//! - [`chat`]: the first-run brief, then the chat with the CEO and attachments.
//! - [`tabs`]: the main panel's tab bar and dots, and where each tab's view is made.
//! - [`team`]: the Team tab, its rows, the drivers list and an agent's view.
//!
//! [`session`] is the one connection, shared by all of them.

pub mod chat;
pub mod input;
pub mod markdown;
pub mod platform;
pub mod session;
pub mod shell;
pub mod sidebar;
pub mod tabs;
pub mod team;
pub mod theme;
pub mod widgets;
pub mod words;

use gpui::{
    App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, px, size,
};
use x100_client::Client;

use crate::session::Session;
use crate::shell::Shell;

/// What the app sets up once, before any window.
pub fn init(cx: &mut App) {
    input::bind_keys(cx);
}

/// Opens the main window on a connected client.
pub fn open_window(cx: &mut App, client: Client) -> gpui::Result<WindowHandle<Shell>> {
    let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("x100".into()),
                ..Default::default()
            }),
            app_id: Some("x100".into()),
            window_min_size: Some(size(px(900.), px(560.))),
            ..Default::default()
        },
        |_, cx| {
            let session = cx.new(|cx| Session::new(client, cx));
            cx.new(|cx| Shell::new(session, cx))
        },
    )
}
