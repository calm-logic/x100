//! The terminal as a GPUI view: fed bytes and sizes, drawn when they change, scrolled with the
//! wheel, selected with the mouse and copied with the platform's copy keys. It sends nothing
//! anywhere: no key, click or focus change reaches the program.
//!
//! **A pane smaller than the recorded size.** A program on a terminal of its own drew at absolute
//! positions for the size it had, and the builder cannot resize it, since the terminal belongs
//! to the daemon. So the cell is scaled down until the whole recorded screen fits, rather than
//! scrolled: the parts that matter most, the input box and the status line, sit at the bottom
//! and the right, where a scrolled view would hide them first. Below [`MIN_FONT`] the text would
//! be unreadable anyway, so the cell stops shrinking there and the right and bottom are clipped.
//! A log the app writes itself is never scaled: its grid takes the pane's size and rewraps.

use std::rc::Rc;
use std::time::Instant;

use alacritty_terminal::index::Side;
use gpui::{
    Bounds, ClipboardItem, Context, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, Render, ScrollEvent, Task, Window, canvas, div, prelude::*, px,
};

use super::paint::{self, Fonts, Frame, Metrics, RowPaint, Shapes};
use super::palette;
use super::{Dirty, Size, TermGrid};
use crate::theme;

/// The terminal's font size when nothing is scaled.
pub const FONT: f32 = 12.;
/// The smallest a recorded screen is scaled to before it is clipped instead.
pub const MIN_FONT: f32 = 6.;
const PAD: f32 = 8.;
/// How much of a log is kept to write again at a new width: about the scrollback's worth.
const KEPT: usize = 2 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sizing {
    /// The grid takes the pane's size: a log the app writes, rewrapped when the pane changes.
    Fit,
    /// The grid keeps the size the program drew at, scaled down to fit the pane.
    Recorded,
}

/// What the view has done, to show that it draws only what changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalStats {
    /// Times the view was rendered.
    pub renders: usize,
    /// Times its grid was laid out for a frame.
    pub frames: usize,
    /// Screen lines turned into paint, summed over frames.
    pub rows_built: usize,
    pub bytes: usize,
}

/// Where the last frame put the grid, to turn a mouse position into a cell.
#[derive(Clone, Copy)]
struct Placed {
    origin: Point<Pixels>,
    metrics: Metrics,
}

pub struct TerminalView {
    grid: TermGrid,
    sizing: Sizing,
    fonts: Fonts,
    shapes: Shapes,
    rows: Vec<Rc<RowPaint>>,
    placed: Option<Placed>,
    /// The cell at [`FONT`], measured once.
    base: Option<Metrics>,
    /// The cell a recorded grid was last scaled to, for the room and grid size it was for.
    scaled: Option<((gpui::Size<Pixels>, Size), Metrics)>,
    stats: TerminalStats,
    scroll_lines: f32,
    selecting: bool,
    focus: FocusHandle,
    sync_timer: Option<Task<()>>,
    /// What a log was fed, to write again when the pane's width changes.
    written: Vec<u8>,
}

impl TerminalView {
    pub fn new(sizing: Sizing, cols: usize, rows: usize, cx: &mut Context<Self>) -> Self {
        Self {
            grid: TermGrid::new(cols, rows),
            sizing,
            fonts: Fonts::new(theme::MONO),
            shapes: Shapes::default(),
            rows: Vec::new(),
            placed: None,
            base: None,
            scaled: None,
            stats: TerminalStats::default(),
            scroll_lines: 0.,
            selecting: false,
            focus: cx.focus_handle(),
            sync_timer: None,
            written: Vec::new(),
        }
    }

    pub fn grid(&self) -> &TermGrid {
        &self.grid
    }

    pub fn sizing(&self) -> Sizing {
        self.sizing
    }

    pub fn stats(&self) -> TerminalStats {
        self.stats
    }

    /// The font size the last frame drew at: below [`FONT`] when a recorded screen was scaled.
    pub fn font_size(&self) -> Option<f32> {
        self.placed.map(|p| f32::from(p.metrics.font_size))
    }

    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        self.stats.bytes += bytes.len();
        if self.sizing == Sizing::Fit {
            self.keep(bytes);
        }
        self.grid.feed(bytes);
        self.arm_sync(cx);
        cx.notify();
    }

    /// Keeps what a log was fed, up to [`KEPT`], dropping whole lines from the front.
    fn keep(&mut self, bytes: &[u8]) {
        self.written.extend_from_slice(bytes);
        if self.written.len() > KEPT {
            let over = self.written.len() - KEPT / 2;
            let cut = self.written[over..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .map_or(over, |i| over + i + 2);
            self.written.drain(..cut);
        }
    }

    /// The size the program's terminal has from here on.
    pub fn resize(&mut self, cols: usize, rows: usize, cx: &mut Context<Self>) {
        self.grid.resize(cols, rows);
        cx.notify();
    }

    /// A program that begins a synchronized update and never ends it would otherwise freeze the
    /// view: what it held back is drawn when the emulator's deadline passes.
    fn arm_sync(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.grid.sync_deadline() else {
            self.sync_timer = None;
            return;
        };
        if self.sync_timer.is_some() {
            return;
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        self.sync_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                v.sync_timer = None;
                v.grid.end_sync();
                cx.notify();
            });
        }));
    }

    /// Lays the grid out in `bounds` and builds the lines that changed since the last frame.
    fn prepare(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> Frame {
        self.stats.frames += 1;
        let inner = Bounds {
            origin: bounds.origin + gpui::point(px(PAD), px(PAD)),
            size: gpui::size(
                (bounds.size.width - px(2. * PAD)).max(px(0.)),
                (bounds.size.height - px(2. * PAD)).max(px(0.)),
            ),
        };
        let base = *self
            .base
            .get_or_insert_with(|| Metrics::measure(&self.fonts, px(FONT), window));
        let metrics = match self.sizing {
            Sizing::Fit => {
                let cols = (inner.size.width / base.cell_width).floor() as usize;
                let rows = (inner.size.height / base.line_height).floor() as usize;
                let (cols, rows) = (cols.max(10), rows.max(2));
                if self.grid.size() != (Size { cols, rows }) {
                    // Written again rather than resized: alacritty's rewrap pushes the lines
                    // it adds into the scrollback, which would hide the top of a short log.
                    self.grid = TermGrid::new(cols, rows);
                    self.grid.feed(&self.written);
                }
                base
            }
            Sizing::Recorded => {
                let key = (inner.size, self.grid.size());
                match self.scaled {
                    Some((k, m)) if k == key => m,
                    _ => {
                        let m = self.scale(base, inner.size, window);
                        self.scaled = Some((key, m));
                        m
                    }
                }
            }
        };
        let mut dirty = self.grid.take_dirty();
        if self.placed.is_none_or(|p| p.metrics != metrics) {
            self.shapes.clear();
            dirty = Dirty::All;
        }
        let term = self.grid.term();
        let screen = self.grid.size().rows;
        if self.rows.len() != screen {
            self.rows.resize_with(screen, Default::default);
            dirty = Dirty::All;
        }
        let lines: Vec<usize> = match dirty {
            Dirty::All => (0..screen).collect(),
            Dirty::Lines(set) => set.into_iter().filter(|l| *l < screen).collect(),
        };
        for line in lines {
            self.rows[line] = Rc::new(paint::build_row(
                term,
                line,
                &metrics,
                &self.fonts,
                &mut self.shapes,
                window,
            ));
            self.stats.rows_built += 1;
        }
        self.placed = Some(Placed {
            origin: inner.origin,
            metrics,
        });
        Frame {
            origin: inner.origin,
            clip: bounds,
            metrics,
            rows: self.rows.clone(),
            background: palette::hex(palette::BACKGROUND),
            cursor: paint::cursor(term),
            selected: paint::selection(term, term.renderable_content().selection),
        }
    }

    /// The largest cell, up to the unscaled one and down to [`MIN_FONT`], at which the recorded
    /// grid fits `room`. Measured at each size tried, since a font's cell does not shrink exactly
    /// in proportion once it is rounded to whole pixels.
    fn scale(&self, base: Metrics, room: gpui::Size<Pixels>, window: &Window) -> Metrics {
        let size = self.grid.size();
        let fits = |m: &Metrics| {
            m.cell_width * size.cols as f32 <= room.width
                && m.line_height * size.rows as f32 <= room.height
        };
        if fits(&base) {
            return base;
        }
        let wide = room.width / (base.cell_width * size.cols as f32);
        let tall = room.height / (base.line_height * size.rows as f32);
        let mut font = (FONT * wide.min(tall)).clamp(MIN_FONT, FONT);
        loop {
            let m = Metrics::measure(&self.fonts, px(font), window);
            if fits(&m) || font <= MIN_FONT {
                return m;
            }
            font = (font - 0.25).max(MIN_FONT);
        }
    }

    /// The cell under a window position, and which half of it.
    fn cell_at(&self, at: Point<Pixels>) -> Option<(usize, usize, Side)> {
        let p = self.placed?;
        let size = self.grid.size();
        let x = (at.x - p.origin.x) / p.metrics.cell_width;
        let y = (at.y - p.origin.y) / p.metrics.line_height;
        let col = (x.max(0.) as usize).min(size.cols - 1);
        let row = (y.max(0.) as usize).min(size.rows - 1);
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        Some((row, col, side))
    }

    fn on_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        if let Some((row, col, side)) = self.cell_at(e.position) {
            let point = self.grid.point(row, col);
            self.grid.select_from(point, side);
            self.selecting = true;
            cx.notify();
        }
    }

    fn on_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || e.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        if let Some((row, col, side)) = self.cell_at(e.position) {
            let point = self.grid.point(row, col);
            self.grid.select_to(point, side);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, e: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.placed else {
            return;
        };
        let line = p.metrics.line_height;
        self.scroll_lines += e.delta.pixel_delta(line).y / line;
        let lines = self.scroll_lines.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_lines -= lines as f32;
        // At either end of the scrollback the wheel goes on to the view around this one.
        if self.grid.scroll(lines) {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = e.keystroke.modifiers;
        let copy = e.keystroke.key == "c" && (m.secondary(cx) || (m.control && m.shift));
        if copy && let Some(text) = self.grid.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.stop_propagation();
        }
    }

    /// Where the last frame drew the middle of a cell, in window coordinates.
    pub fn cell_center(&self, row: usize, col: usize) -> Option<Point<Pixels>> {
        let p = self.placed?;
        Some(gpui::point(
            p.origin.x + p.metrics.cell_width * (col as f32 + 0.5),
            p.origin.y + p.metrics.line_height * (row as f32 + 0.5),
        ))
    }

    /// The selected text, as copying it would give.
    pub fn selected_text(&self) -> Option<String> {
        self.grid.selected_text()
    }

    /// Scrolls the scrollback by whole lines, up for positive, as the wheel does.
    pub fn scroll(&mut self, lines: i32, cx: &mut Context<Self>) -> bool {
        let moved = self.grid.scroll(lines);
        if moved {
            cx.notify();
        }
        moved
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.stats.renders += 1;
        let this = cx.entity();
        div()
            .id("terminal")
            .debug_selector(|| "terminal".into())
            .track_focus(&self.focus)
            .size_full()
            .bg(palette::hex(palette::BACKGROUND))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .on_key_down(cx.listener(Self::on_key))
            .child(
                canvas(
                    move |bounds, window, cx| this.update(cx, |v, _| v.prepare(bounds, window)),
                    |_, frame, window, _| paint::paint(&frame, window),
                )
                .size_full(),
            )
    }
}
