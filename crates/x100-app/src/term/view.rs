//! The terminal as a GPUI view: fed bytes and sizes, drawn when they change, scrolled with the
//! wheel, selected with the mouse and copied with the platform's copy keys. It sends nothing to
//! the program: no key, click or focus change reaches it.
//!
//! **Always at [`FONT`], never scaled.** A program's screen shrunk to fit a pane is unreadable, so
//! a recorded terminal is drawn cell for cell at the normal size, and the program is asked to draw
//! at the pane's size instead: the view announces how many cells its pane holds
//! ([`PaneCells`]) once that has held for [`SETTLE`], and whoever owns the view asks the daemon to
//! resize the program's terminal. Until the program redraws, or for good once its driver has
//! ended, a screen bigger than the pane is cut off and moved with the wheel, starting at its
//! bottom, where both programs keep their input box and status line. A log the app writes itself
//! takes the pane's size and rewraps.

use std::rc::Rc;
use std::time::{Duration, Instant};

use alacritty_terminal::index::Side;
use gpui::{
    Bounds, ClipboardItem, Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, ScrollEvent, Task, Window, canvas, div,
    prelude::*, px,
};

use super::paint::{self, Fonts, Frame, Metrics, RowPaint, Shapes};
use super::palette;
use super::{Dirty, Size, TermGrid};
use crate::theme;

/// The terminal's font size, the app's normal monospace size.
pub const FONT: f32 = 12.;
/// How long a pane's size in cells must hold before it is announced: long enough that dragging a
/// window's edge announces only where it stops.
pub const SETTLE: Duration = Duration::from_millis(250);
const PAD: f32 = 8.;
/// How much of a log is kept to write again at a new width: about the scrollback's worth.
const KEPT: usize = 2 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sizing {
    /// The grid takes the pane's size: a log the app writes, rewrapped when the pane changes.
    Fit,
    /// The grid keeps the size the program drew at, at [`FONT`], and the pane's size in cells is
    /// announced so the program can be asked to draw at it.
    Recorded,
}

/// How many cells the pane of a [`Sizing::Recorded`] view holds, announced when that changes and
/// has held for [`SETTLE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneCells {
    pub cols: u16,
    pub rows: u16,
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
    /// How far the grid can be moved within the pane, when it is bigger than the pane.
    overflow: Point<Pixels>,
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
    /// How far a grid bigger than its pane is moved, from its top left. It starts past the bottom
    /// and is held inside the grid when drawn, so the bottom shows until the wheel moves it.
    pan: Point<Pixels>,
    /// What the pane holds now, and what was last announced.
    pane: Option<PaneCells>,
    announced: Option<PaneCells>,
    announce: Option<Task<()>>,
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
            pan: gpui::point(px(0.), px(f32::MAX)),
            pane: None,
            announced: None,
            announce: None,
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

    /// The font size the last frame drew at.
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

    /// What the pane held when the last frame was drawn, in cells, for a recorded grid.
    pub fn pane(&self) -> Option<PaneCells> {
        self.pane
    }

    /// Where the grid's top left is drawn, from the pane's: zero unless it is moved within a pane
    /// smaller than it.
    pub fn panned(&self) -> Option<Point<Pixels>> {
        Some(held(self.pan, self.placed?.overflow))
    }

    /// Notes the cells `room` holds, and announces them once they have held for [`SETTLE`]. A size
    /// that changes and changes back before then is not announced at all.
    fn measure_pane(&mut self, room: gpui::Size<Pixels>, cell: Metrics, cx: &mut Context<Self>) {
        let fit = |room: Pixels, cell: Pixels| (room / cell).floor().clamp(0., f32::from(u16::MAX));
        let cells = PaneCells {
            cols: fit(room.width, cell.cell_width) as u16,
            rows: fit(room.height, cell.line_height) as u16,
        };
        if self.pane == Some(cells) {
            return;
        }
        self.pane = Some(cells);
        self.announce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |v, cx| {
                v.announce = None;
                if v.pane != v.announced {
                    v.announced = v.pane;
                    cx.emit(cells);
                }
            });
        }));
    }

    /// Lays the grid out in `bounds` and builds the lines that changed since the last frame.
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Frame {
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
                self.measure_pane(inner.size, base, cx);
                base
            }
        };
        let size = self.grid.size();
        let overflow = gpui::point(
            (metrics.cell_width * size.cols as f32 - inner.size.width).max(px(0.)),
            (metrics.line_height * size.rows as f32 - inner.size.height).max(px(0.)),
        );
        let origin = inner.origin - held(self.pan, overflow);
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
            origin,
            metrics,
            overflow,
        });
        Frame {
            origin,
            clip: bounds,
            content: inner,
            metrics,
            rows: self.rows.clone(),
            background: palette::hex(palette::BACKGROUND),
            cursor: paint::cursor(term),
            selected: paint::selection(term, term.renderable_content().selection),
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
        // A screen bigger than its pane is moved over first: the program drew it whole.
        if p.overflow != Point::default() {
            let delta = e.delta.pixel_delta(line);
            let now = self.panned().unwrap_or_default();
            let to = gpui::point(
                (now.x - delta.x).clamp(px(0.), p.overflow.x),
                (now.y - delta.y).clamp(px(0.), p.overflow.y),
            );
            if to != now {
                self.pan = to;
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
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

/// `pan` held inside how far the grid can move.
fn held(pan: Point<Pixels>, overflow: Point<Pixels>) -> Point<Pixels> {
    gpui::point(pan.x.min(overflow.x), pan.y.min(overflow.y))
}

impl EventEmitter<PaneCells> for TerminalView {}

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
                    move |bounds, window, cx| {
                        this.update(cx, |v, cx| v.prepare(bounds, window, cx))
                    },
                    |_, frame, window, _| paint::paint(&frame, window),
                )
                .size_full(),
            )
    }
}
