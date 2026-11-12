//! Spike S1. One window with the three things the x100 desktop app depends on: a list that
//! updates in place, a Markdown view and a live terminal on a PTY.
//!
//! Environment:
//! - `SPIKE_SECONDS`: quit after this many seconds (default: run until the window closes).
//! - `SPIKE_TICK_MS`, `SPIKE_ROWS_PER_TICK`: how often the list changes and how many rows.
//! - `SPIKE_UNCACHED=1`: embed rows without view caching, to show what caching saves.
//! - `SPIKE_CACHE_LIST=1`: also cache the list itself, which defeats the row caches.
//! - `SPIKE_RESIZE_AT`: shrink the window from code after this many seconds.
//! - `SPIKE_WINDOWING=x11|wayland`: choose the display protocol in code, whatever the environment.
//! - `SPIKE_TERM_CMD`: the program for the terminal, run through `sh -c`.

mod list;
mod markdown;
mod stats;
mod terminal;

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, Context, Entity, IntoElement, ParentElement, Render, StyleRefinement, Styled,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, canvas, div, prelude::*,
    px, rgb, size,
};
use gpui_platform::application;

const LIST_WIDTH: f32 = 380.;
const MARKDOWN_WIDTH: f32 = 520.;
const HEADER_HEIGHT: f32 = 28.;

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct StatsBar {
    started: Instant,
}

impl Render for StatsBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .px_2()
            .flex()
            .items_center()
            .bg(rgb(0x161b22))
            .text_color(rgb(0x9198a1))
            .font_family("DejaVu Sans Mono")
            .text_size(px(12.))
            .child(format!(
                "t={:>4}s  {}",
                self.started.elapsed().as_secs(),
                stats::line()
            ))
    }
}

struct Root {
    stats: Entity<StatsBar>,
    list: Entity<list::ListView>,
    markdown: Entity<markdown::MarkdownView>,
    terminal: Entity<terminal::TerminalView>,
    cache_list: bool,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        stats::bump(&stats::ROOT_RENDERS);
        let column = |w: f32| {
            div()
                .w(px(w))
                .h_full()
                .flex_none()
                .border_r_1()
                .border_color(rgb(0x30363d))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x0d1117))
            .text_color(rgb(0xd1d7e0))
            .font_family("DejaVu Sans")
            .child(
                self.stats
                    .clone()
                    .cached(StyleRefinement::default().w_full().h(px(HEADER_HEIGHT))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    // The list is embedded uncached on purpose. A cached view that is dirty
                    // re-renders its whole subtree, cached children included, and a row's
                    // notify dirties every ancestor view. Uncached, the list rebuilds 100 cheap
                    // view handles per frame and each row reuses its cache unless it changed.
                    .child(column(LIST_WIDTH).child(if self.cache_list {
                        self.list
                            .clone()
                            .cached(StyleRefinement::default().size_full())
                            .into_any_element()
                    } else {
                        self.list.clone().into_any_element()
                    }))
                    .child(
                        column(MARKDOWN_WIDTH).child(
                            self.markdown
                                .clone()
                                .cached(StyleRefinement::default().size_full()),
                        ),
                    )
                    .child(
                        div().flex_1().h_full().min_w_0().child(
                            self.terminal
                                .clone()
                                .cached(StyleRefinement::default().size_full()),
                        ),
                    ),
            )
            // Paints once per drawn frame, whatever was or was not re-rendered.
            .child(
                canvas(|_, _, _| {}, |_, _, _, _| stats::bump(&stats::FRAMES))
                    .absolute()
                    .size_0(),
            )
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let seconds: u64 = env_or("SPIKE_SECONDS", 0);
    let tick = Duration::from_millis(env_or("SPIKE_TICK_MS", 20));
    let per_tick: usize = env_or("SPIKE_ROWS_PER_TICK", 3);
    let cached = std::env::var("SPIKE_UNCACHED").is_err();
    let cache_list = std::env::var("SPIKE_CACHE_LIST").is_ok();
    let term_cmd = std::env::var("SPIKE_TERM_CMD")
        .unwrap_or_else(|_| format!("bash {}/feed.sh", env!("CARGO_MANIFEST_DIR")));
    log::info!(
        "spike: tick={tick:?} rows_per_tick={per_tick} cached_rows={cached} cache_list={cache_list} term_cmd={term_cmd:?} seconds={seconds}"
    );

    let resize_at: u64 = env_or("SPIKE_RESIZE_AT", 0);
    // Under WSLg only X11 reaches the GPU (see REPORT.md), so the app can pick it in code.
    let app = match std::env::var("SPIKE_WINDOWING").as_deref() {
        Ok("x11") => gpui_platform::linux(gpui::WindowingModes::X11),
        Ok("wayland") => gpui_platform::linux(gpui::WindowingModes::WAYLAND),
        _ => application(),
    };
    app.run(move |cx: &mut App| {
        // The timer task is spawned while the window is built, before its handle exists.
        let window_slot: Rc<RefCell<Option<WindowHandle<Root>>>> = Rc::default();
        let window_handle = window_slot.clone();
        let bounds = Bounds::centered(None, size(px(1600.), px(960.)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("x100 GPUI spike".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_window, cx| {
                    let started = Instant::now();
                    let stats_bar = cx.new(|_| StatsBar { started });
                    let list = cx.new(|cx| list::ListView::new(100, tick, per_tick, cached, cx));
                    let markdown =
                        cx.new(|_| markdown::MarkdownView::new(include_str!("../sample.md")));
                    let terminal = cx.new(|cx| {
                        terminal::TerminalView::new(
                            "sh".into(),
                            vec!["-c".into(), term_cmd.clone()],
                            cx,
                        )
                    });

                    let bar = stats_bar.clone();
                    cx.spawn(async move |cx| {
                        let mut last_log = Instant::now();
                        let mut resized = false;
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(500))
                                .await;
                            cx.update(|cx| bar.update(cx, |_, cx| cx.notify()));
                            if resize_at > 0
                                && !resized
                                && started.elapsed() >= Duration::from_secs(resize_at)
                            {
                                resized = true;
                                eprintln!(
                                    "STATS before-resize t={}s {}",
                                    started.elapsed().as_secs(),
                                    stats::line()
                                );
                                let handle = window_handle.borrow().expect("window handle");
                                let _ = handle.update(cx, |_, window, _| {
                                    window.resize(size(px(1300.), px(760.)))
                                });
                            }
                            if last_log.elapsed() >= Duration::from_secs(5) {
                                last_log = Instant::now();
                                eprintln!(
                                    "STATS t={}s {}",
                                    started.elapsed().as_secs(),
                                    stats::line()
                                );
                            }
                            if seconds > 0 && started.elapsed() >= Duration::from_secs(seconds) {
                                eprintln!(
                                    "STATS final t={}s {}",
                                    started.elapsed().as_secs(),
                                    stats::line()
                                );
                                cx.update(|cx| cx.quit());
                                break;
                            }
                        }
                    })
                    .detach();

                    cx.new(|_| Root {
                        stats: stats_bar,
                        list,
                        markdown,
                        terminal,
                        cache_list,
                    })
                },
            )
            .expect("open window");
        window_slot.replace(Some(handle));
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
