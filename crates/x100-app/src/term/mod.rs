//! A terminal for watching, not for typing into: an alacritty_terminal grid fed bytes and resizes
//! that the daemon recorded, and a view that paints it.
//!
//! The terminal belongs to the daemon. Its own emulator answered the program's queries live, so
//! nothing here ever writes back: the grid's listener swallows every reply the emulator would send
//! (device attributes, cursor reports, colours, clipboard), and the view forwards no keys, mouse
//! or focus to the program, whatever modes it asks for.
//!
//! - [`TermGrid`]: the emulator and what changed since it was last painted.
//! - [`TerminalView`]: the GPUI view, with scrollback, selection and copy.
//! - `paint`: the cells, drawn one by one at fixed positions.
//! - `palette`: the 16, 256 and true colours.

mod paint;
mod palette;
mod view;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, Term, TermDamage};
use alacritty_terminal::vte::ansi::Processor;

pub use palette::{BACKGROUND, FOREGROUND};
pub use view::{FONT, MIN_FONT, Sizing, TerminalStats, TerminalView};

/// How many lines a log keeps above the screen.
const HISTORY: usize = 10_000;

/// The grid's size, as alacritty asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Takes every event the emulator raises and acts on none of them. A reply to the program would
/// reach no one, a clipboard write or a bell would act on the builder's own desktop, and a title
/// belongs to the program's terminal, not the app's window: only the title is kept, to show.
#[derive(Clone, Default)]
pub struct Quiet(Arc<QuietState>);

#[derive(Default)]
struct QuietState {
    swallowed: AtomicUsize,
    title: Mutex<Option<String>>,
}

impl EventListener for Quiet {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(_)
            | Event::ClipboardLoad(..)
            | Event::ClipboardStore(..)
            | Event::ColorRequest(..)
            | Event::TextAreaSizeRequest(_) => {
                self.0.swallowed.fetch_add(1, Ordering::Relaxed);
            }
            Event::Title(title) => *self.0.title.lock().expect("title lock") = Some(title),
            Event::ResetTitle => *self.0.title.lock().expect("title lock") = None,
            _ => {}
        }
    }
}

/// Which screen lines need building again before the next paint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dirty {
    All,
    Lines(BTreeSet<usize>),
}

impl Dirty {
    fn add(&mut self, damage: TermDamage<'_>) {
        match (damage, &mut *self) {
            (_, Dirty::All) => {}
            (TermDamage::Full, _) => *self = Dirty::All,
            (TermDamage::Partial(lines), Dirty::Lines(set)) => set.extend(lines.map(|l| l.line)),
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Dirty::Lines(set) if set.is_empty())
    }
}

/// The emulator, fed only what the program wrote.
pub struct TermGrid {
    term: Term<Quiet>,
    parser: Processor,
    quiet: Quiet,
    dirty: Dirty,
}

impl TermGrid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let quiet = Quiet::default();
        let config = Config {
            scrolling_history: HISTORY,
            // Neither is ever answered, so neither is turned on.
            osc52: Osc52::Disabled,
            kitty_keyboard: false,
            ..Config::default()
        };
        let size = Size {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        Self {
            term: Term::new(config, &size, quiet.clone()),
            parser: Processor::new(),
            quiet,
            dirty: Dirty::All,
        }
    }

    /// Takes bytes as the program wrote them, split anywhere.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.collect_damage();
    }

    /// When a synchronized update the program began must be drawn even if it never ends it.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Draws what a synchronized update has held back so far.
    pub fn end_sync(&mut self) {
        if self.sync_deadline().is_some() {
            self.parser.stop_sync(&mut self.term);
            self.collect_damage();
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let size = Size {
            cols: cols.max(1),
            rows: rows.max(1),
        };
        if size != self.size() {
            self.term.resize(size);
            self.dirty = Dirty::All;
            self.term.reset_damage();
        }
    }

    pub fn size(&self) -> Size {
        Size {
            cols: self.term.columns(),
            rows: self.term.screen_lines(),
        }
    }

    fn collect_damage(&mut self) {
        self.dirty.add(self.term.damage());
        self.term.reset_damage();
    }

    /// The lines to build again, and from now on none.
    pub fn take_dirty(&mut self) -> Dirty {
        std::mem::replace(&mut self.dirty, Dirty::Lines(BTreeSet::new()))
    }

    pub fn term(&self) -> &Term<Quiet> {
        &self.term
    }

    /// How many replies to the program were dropped: answers to its queries and the like.
    pub fn swallowed(&self) -> usize {
        self.quiet.0.swallowed.load(Ordering::Relaxed)
    }

    pub fn title(&self) -> Option<String> {
        self.quiet.0.title.lock().expect("title lock").clone()
    }

    /// The screen as text, one string per line, trailing blanks trimmed.
    pub fn screen_text(&self) -> Vec<String> {
        screen_text(&self.term)
    }

    /// The lines on view, which are the screen's unless the scrollback is scrolled.
    pub fn visible_text(&self) -> Vec<String> {
        let offset = self.display_offset() as i32;
        (0..self.term.screen_lines() as i32)
            .map(|l| {
                line_text(&self.term, Line(l - offset))
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// Everything still held, the scrollback first, as text: a line the terminal wrapped is one
    /// line here.
    pub fn all_text(&self) -> String {
        let grid = self.term.grid();
        let last = Column(self.term.columns() - 1);
        let mut out = String::new();
        for line in grid.topmost_line().0..=grid.bottommost_line().0 {
            let text = line_text(&self.term, Line(line));
            if grid[Line(line)][last].flags.contains(Flags::WRAPLINE) {
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end());
                out.push('\n');
            }
        }
        out
    }

    /// Scrolls the scrollback by `lines`, up for positive. Returns whether the view moved.
    pub fn scroll(&mut self, lines: i32) -> bool {
        let before = self.term.grid().display_offset();
        self.term.scroll_display(Scroll::Delta(lines));
        let moved = self.term.grid().display_offset() != before;
        if moved {
            self.dirty = Dirty::All;
        }
        moved
    }

    /// How far up the scrollback the view is.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// The grid point under screen line `row` and column `col`.
    pub fn point(&self, row: usize, col: usize) -> Point {
        let offset = self.display_offset() as i32;
        let col = col.min(self.term.columns().saturating_sub(1));
        Point::new(Line(row as i32 - offset), Column(col))
    }

    pub fn select_from(&mut self, point: Point, side: Side) {
        self.term.selection = Some(Selection::new(SelectionType::Simple, point, side));
    }

    pub fn select_to(&mut self, point: Point, side: Side) {
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }
}

/// A screen's text, for any listener: the widget's and a bare replay's are compared by it.
pub fn screen_text<T>(term: &Term<T>) -> Vec<String> {
    (0..term.screen_lines() as i32)
        .map(|line| line_text(term, Line(line)).trim_end().to_owned())
        .collect()
}

fn line_text<T>(term: &Term<T>, line: Line) -> String {
    let row = &term.grid()[line];
    let mut text = String::new();
    for col in 0..term.columns() {
        let cell = &row[Column(col)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        text.push(if cell.c == '\0' { ' ' } else { cell.c });
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_swallowed_and_the_title_kept() {
        let mut grid = TermGrid::new(20, 4);
        // Primary device attributes, cursor position report, a colour query, an OSC 52 write.
        grid.feed(b"\x1b[c\x1b[6n\x1b]10;?\x07\x1b]52;c;aGk=\x07\x1b]0;Claude Code\x07hello");
        assert!(grid.swallowed() >= 3, "{}", grid.swallowed());
        assert_eq!(grid.title().as_deref(), Some("Claude Code"));
        assert_eq!(grid.screen_text()[0], "hello");
    }

    #[test]
    fn a_line_change_dirties_that_line_only() {
        let mut grid = TermGrid::new(20, 6);
        grid.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(grid.take_dirty(), Dirty::All);
        grid.feed(b"\x1b[2;1Hxyz");
        let Dirty::Lines(lines) = grid.take_dirty() else {
            panic!("everything dirtied")
        };
        // The cursor's old line and its new one.
        assert_eq!(lines, BTreeSet::from([1, 2]));
        assert_eq!(grid.screen_text()[1], "xyz");
    }

    #[test]
    fn a_held_synchronized_update_is_drawn_when_ended() {
        let mut grid = TermGrid::new(20, 3);
        grid.feed(b"\x1b[?2026hheld");
        assert!(grid.sync_deadline().is_some());
        assert_eq!(grid.screen_text()[0], "");
        grid.end_sync();
        assert_eq!(grid.screen_text()[0], "held");
    }

    #[test]
    fn selection_is_copied_as_text() {
        let mut grid = TermGrid::new(20, 3);
        grid.feed(b"copy this line");
        grid.select_from(grid.point(0, 5), Side::Left);
        grid.select_to(grid.point(0, 8), Side::Right);
        assert_eq!(grid.selected_text().as_deref(), Some("this"));
        grid.clear_selection();
        assert_eq!(grid.selected_text(), None);
    }
}
