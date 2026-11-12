//! The terminal widget against real recordings, on GPUI's test platform with no display.
//!
//! `tests/data` holds two terminals recorded by M3T's research: the real `claude` (`claude.raw`)
//! and `codex` (`codex.raw`) programs at 120x40, run against a fake provider with dummy logins
//! inside a scratch directory, each with its timing (`seconds bytes` per read). Set
//! `X100_RECORDINGS` to a directory of such runs (`*/term.raw` with `term.raw.timing`) to replay
//! more of them too.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use gpui::{
    Entity, Modifiers, MouseButton, ScrollDelta, ScrollEvent, StyleRefinement, TestAppContext,
    VisualTestContext, Window, div, point, prelude::*, px, size,
};
use x100_app::term::{FONT, PaneCells, SETTLE, Size, Sizing, TerminalView, screen_text};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

struct Recording {
    name: String,
    bytes: Vec<u8>,
    /// The sizes of the reads the program's output arrived in.
    reads: Vec<usize>,
}

fn load(raw: &Path, timing: &Path, name: String) -> Recording {
    let bytes = std::fs::read(raw).expect("a recording");
    let reads = std::fs::read_to_string(timing)
        .expect("its timing")
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.parse().ok())
        .collect::<Vec<usize>>();
    assert_eq!(reads.iter().sum::<usize>(), bytes.len(), "{name}");
    Recording { name, bytes, reads }
}

fn recordings() -> Vec<Recording> {
    let mut all: Vec<Recording> = ["claude", "codex"]
        .map(|n| {
            let dir = Path::new(DATA);
            load(
                &dir.join(format!("{n}.raw")),
                &dir.join(format!("{n}.timing")),
                n.into(),
            )
        })
        .into();
    if let Some(dir) = std::env::var_os("X100_RECORDINGS") {
        let mut runs: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("X100_RECORDINGS is a directory")
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.join("term.raw.timing").exists())
            .collect();
        runs.sort();
        all.extend(runs.iter().map(|run| {
            let name = run.file_name().unwrap().to_string_lossy().into_owned();
            load(&run.join("term.raw"), &run.join("term.raw.timing"), name)
        }));
    }
    all
}

/// Counts what a terminal would write back to the program.
#[derive(Clone, Default)]
struct Replies(std::rc::Rc<std::cell::Cell<usize>>);

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        if matches!(
            event,
            Event::PtyWrite(_)
                | Event::ColorRequest(..)
                | Event::TextAreaSizeRequest(_)
                | Event::ClipboardLoad(..)
                | Event::ClipboardStore(..)
        ) {
            self.0.set(self.0.get() + 1);
        }
    }
}

/// The recording replayed into a bare emulator, with nothing between: what the widget must show.
fn bare(bytes: &[u8]) -> (Term<Replies>, usize) {
    let replies = Replies::default();
    let mut term = Term::new(
        Config::default(),
        &Size {
            cols: 120,
            rows: 40,
        },
        replies.clone(),
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut term, bytes);
    if parser.sync_timeout().sync_timeout().is_some() {
        parser.stop_sync(&mut term);
    }
    let n = replies.0.get();
    (term, n)
}

/// Every cell of the screen with its colours and attributes.
fn cells<T>(term: &Term<T>) -> Vec<String> {
    (0..term.screen_lines() as i32)
        .flat_map(|l| {
            let row = &term.grid()[Line(l)];
            (0..term.columns())
                .map(|c| {
                    let cell = &row[Column(c)];
                    format!("{:?}{:?}{:?}{:?}", cell.c, cell.fg, cell.bg, cell.flags)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn window(cx: &mut TestAppContext, sizing: Sizing) -> (Entity<TerminalView>, VisualTestContext) {
    let (view, vcx) = cx.add_window_view(|_, cx| TerminalView::new(sizing, 120, 40, cx));
    let vcx = vcx.clone();
    (view, vcx)
}

/// Feeds `bytes` in pieces of the given sizes, then lets any held update show.
fn replay(
    view: &Entity<TerminalView>,
    vcx: &mut VisualTestContext,
    bytes: &[u8],
    pieces: &[usize],
) {
    let mut at = 0;
    for n in pieces.iter().copied().cycle() {
        if at >= bytes.len() {
            break;
        }
        let end = (at + n).min(bytes.len());
        vcx.update(|_, cx| view.update(cx, |v, cx| v.feed(&bytes[at..end], cx)));
        at = end;
    }
    vcx.run_until_parked();
    vcx.executor().advance_clock(Duration::from_secs(1));
    vcx.run_until_parked();
}

#[gpui::test]
fn real_recordings_replay_as_a_bare_terminal_shows_them(cx: &mut TestAppContext) {
    for rec in recordings() {
        let (view, mut vcx) = window(cx, Sizing::Recorded);
        replay(&view, &mut vcx, &rec.bytes, &rec.reads);
        let (want, replies) = bare(&rec.bytes);
        vcx.read(|cx| {
            let grid = view.read(cx).grid();
            assert_eq!(grid.screen_text(), screen_text(&want), "{}", rec.name);
            assert_eq!(cells(grid.term()), cells(&want), "{}", rec.name);
            // Each reply the program asked for was raised and dropped: none can leave the view.
            assert_eq!(grid.swallowed(), replies, "{}", rec.name);
        });
        let text = vcx.read(|cx| view.read(cx).grid().screen_text().join("\n"));
        assert!(!text.trim().is_empty(), "{} replayed to nothing", rec.name);
    }
    // Both programs ask their terminal questions, which a bare replay would answer.
    let claude = recordings().remove(0);
    assert!(bare(&claude.bytes).1 > 0);
}

#[gpui::test]
fn the_same_bytes_in_other_pieces_give_the_same_grid(cx: &mut TestAppContext) {
    for rec in recordings().into_iter().take(2) {
        let mut seen = Vec::new();
        for pieces in [vec![1], vec![3, 7], vec![64], vec![4096], rec.reads.clone()] {
            let (view, mut vcx) = window(cx, Sizing::Recorded);
            replay(&view, &mut vcx, &rec.bytes, &pieces);
            seen.push(vcx.read(|cx| cells(view.read(cx).grid().term())));
        }
        assert!(seen.windows(2).all(|w| w[0] == w[1]), "{}", rec.name);
    }
}

#[gpui::test]
fn a_change_rebuilds_only_its_lines(cx: &mut TestAppContext) {
    let rec = recordings().remove(0);
    let (view, mut vcx) = window(cx, Sizing::Recorded);
    replay(&view, &mut vcx, &rec.bytes, &rec.reads);
    let before = vcx.read(|cx| view.read(cx).stats());
    assert!(before.frames > 0 && before.rows_built >= 40, "{before:?}");
    // One line written somewhere on the screen.
    replay(&view, &mut vcx, b"\x1b[12;3Hone line", &[64]);
    let after = vcx.read(|cx| view.read(cx).stats());
    assert_eq!(after.renders, before.renders + 1, "{before:?} {after:?}");
    // The line written, and the cursor's line before and after.
    assert!(
        after.rows_built - before.rows_built <= 3,
        "{before:?} {after:?}"
    );
    // Nothing fed, nothing drawn.
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| view.read(cx).stats()), after);
}

/// Two terminals side by side, each a cached view.
struct Two {
    a: Entity<TerminalView>,
    b: Entity<TerminalView>,
}

impl Render for Two {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let style = || StyleRefinement::default().w(px(500.)).h(px(400.));
        div()
            .flex()
            .child(self.a.clone().cached(style()))
            .child(self.b.clone().cached(style()))
    }
}

#[gpui::test]
fn output_in_one_terminal_leaves_another_alone(cx: &mut TestAppContext) {
    let (two, vcx) = cx.add_window_view(|_, cx| Two {
        a: cx.new(|cx| TerminalView::new(Sizing::Fit, 80, 24, cx)),
        b: cx.new(|cx| TerminalView::new(Sizing::Fit, 80, 24, cx)),
    });
    let mut vcx = vcx.clone();
    let (a, b) = vcx.read(|cx| (two.read(cx).a.clone(), two.read(cx).b.clone()));
    vcx.run_until_parked();
    let quiet = vcx.read(|cx| b.read(cx).stats());
    for i in 0..20 {
        replay(&a, &mut vcx, format!("line {i}\r\n").as_bytes(), &[64]);
    }
    assert_eq!(vcx.read(|cx| b.read(cx).stats()), quiet);
    assert!(vcx.read(|cx| a.read(cx).stats().renders) > 20);
}

/// Each pane size the view announces, in order.
fn announced(
    view: &Entity<TerminalView>,
    vcx: &mut VisualTestContext,
) -> (Rc<RefCell<Vec<PaneCells>>>, gpui::Subscription) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let keep = Rc::clone(&seen);
    let subscription = vcx.update(|_, cx| {
        cx.subscribe(view, move |_, cells: &PaneCells, _| {
            keep.borrow_mut().push(*cells)
        })
    });
    (seen, subscription)
}

/// Lets a pane size settle: past [`SETTLE`] on the test clock.
fn settle(vcx: &mut VisualTestContext) {
    vcx.run_until_parked();
    vcx.executor()
        .advance_clock(SETTLE + Duration::from_millis(10));
    vcx.run_until_parked();
}

#[gpui::test]
fn the_view_asks_for_its_panes_size_in_cells_once_and_again_only_when_it_changes(
    cx: &mut TestAppContext,
) {
    let (view, mut vcx) = window(cx, Sizing::Recorded);
    let (seen, _keep) = announced(&view, &mut vcx);
    vcx.simulate_resize(size(px(1000.), px(600.)));
    vcx.run_until_parked();
    assert!(seen.borrow().is_empty(), "announced before the size held");
    settle(&mut vcx);
    let first = vcx.read(|cx| view.read(cx).pane()).unwrap();
    assert_eq!(*seen.borrow(), [first]);
    // The cells the pane holds at the normal font: as many as fit, inside the padding.
    let cell = vcx
        .read(|cx| {
            view.read(cx)
                .cell_center(0, 1)
                .zip(view.read(cx).cell_center(1, 0))
        })
        .map(|(right, below)| {
            let origin = vcx.read(|cx| view.read(cx).cell_center(0, 0)).unwrap();
            (right.x - origin.x, below.y - origin.y)
        })
        .unwrap();
    let fits = |room: f32, cell: gpui::Pixels| (px(room - 16.) / cell).floor() as u16;
    assert_eq!(first.cols, fits(1000., cell.0), "{first:?}");
    assert_eq!(first.rows, fits(600., cell.1), "{first:?}");
    assert_eq!(vcx.read(|cx| view.read(cx).font_size()), Some(FONT));

    // Frames and time without a change announce nothing more.
    for _ in 0..3 {
        vcx.update(|window, _| window.refresh());
        settle(&mut vcx);
    }
    assert_eq!(seen.borrow().len(), 1);

    // A window edge dragged through several sizes announces only where it stops.
    for width in [950., 900., 850., 800.] {
        vcx.simulate_resize(size(px(width), px(500.)));
        vcx.run_until_parked();
    }
    settle(&mut vcx);
    let stopped = vcx.read(|cx| view.read(cx).pane()).unwrap();
    assert_eq!(*seen.borrow(), [first, stopped]);
    assert!(stopped.cols < first.cols && stopped.rows < first.rows);

    // A pixel less that is not a cell less announces nothing.
    vcx.simulate_resize(size(px(799.), px(499.)));
    settle(&mut vcx);
    assert_eq!(vcx.read(|cx| view.read(cx).pane()), Some(stopped));
    assert_eq!(seen.borrow().len(), 2, "{:?} cell {cell:?}", seen.borrow());
    // Changed and changed back before it held: nothing either.
    vcx.simulate_resize(size(px(600.), px(400.)));
    vcx.run_until_parked();
    vcx.simulate_resize(size(px(800.), px(500.)));
    settle(&mut vcx);
    assert_eq!(seen.borrow().len(), 2);
}

#[gpui::test]
fn a_recorded_screen_bigger_than_its_pane_is_shown_unshrunk_from_its_bottom(
    cx: &mut TestAppContext,
) {
    // A driver that has ended cannot redraw at the pane's size: its screen stays as recorded.
    let (view, mut vcx) = window(cx, Sizing::Recorded);
    let rec = recordings().remove(0);
    replay(&view, &mut vcx, &rec.bytes, &rec.reads);
    vcx.simulate_resize(size(px(500.), px(300.)));
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| view.read(cx).font_size()), Some(FONT));
    assert_eq!(
        vcx.read(|cx| view.read(cx).grid().size()),
        Size {
            cols: 120,
            rows: 40
        }
    );
    // Its bottom row is in view, at the normal size; its top is above the pane.
    let pane = vcx.update(|window, _| window.viewport_size());
    let last = vcx.read(|cx| view.read(cx).cell_center(39, 0)).unwrap();
    let first = vcx.read(|cx| view.read(cx).cell_center(0, 0)).unwrap();
    assert!(
        last.y < pane.height && last.y > pane.height - px(40.),
        "{last:?}"
    );
    assert!(first.y < px(0.), "{first:?}");
    let bottom = vcx.read(|cx| view.read(cx).panned()).unwrap();
    assert!(bottom.y > px(0.) && bottom.x == px(0.), "{bottom:?}");

    // The wheel moves the pane over the screen, up and to the right, and no further than its edges.
    vcx.simulate_event(ScrollEvent {
        position: last,
        delta: ScrollDelta::Lines(point(0., 5.)),
        ..Default::default()
    });
    vcx.run_until_parked();
    let up = vcx.read(|cx| view.read(cx).panned()).unwrap();
    assert!(up.y < bottom.y, "{up:?} {bottom:?}");
    vcx.simulate_event(ScrollEvent {
        position: last,
        delta: ScrollDelta::Pixels(point(px(-10_000.), px(10_000.))),
        ..Default::default()
    });
    vcx.run_until_parked();
    let corner = vcx.read(|cx| view.read(cx).panned()).unwrap();
    assert_eq!(corner.y, px(0.));
    assert!(corner.x > px(0.));
    let right = vcx.read(|cx| view.read(cx).cell_center(0, 119)).unwrap();
    assert!(
        right.x < pane.width,
        "the last column is in view: {right:?}"
    );
    // Nothing of the screen's own scrollback moved: the screen is all there is.
    assert_eq!(vcx.read(|cx| view.read(cx).grid().display_offset()), 0);
}

#[gpui::test]
fn the_wheel_scrolls_back_and_a_selection_is_copied(cx: &mut TestAppContext) {
    let (view, mut vcx) = window(cx, Sizing::Fit);
    let text: String = (0..200)
        .map(|i| format!("line {i} of the log\r\n"))
        .collect();
    replay(&view, &mut vcx, text.as_bytes(), &[512]);
    let rows = vcx.read(|cx| view.read(cx).grid().size().rows);
    let last = vcx.read(|cx| view.read(cx).grid().screen_text());
    assert_eq!(last[rows - 2], "line 199 of the log");

    let middle = vcx.read(|cx| view.read(cx).cell_center(2, 2)).unwrap();
    vcx.simulate_event(ScrollEvent {
        position: middle,
        delta: ScrollDelta::Lines(point(0., 3.)),
        ..Default::default()
    });
    vcx.run_until_parked();
    assert_eq!(vcx.read(|cx| view.read(cx).grid().display_offset()), 3);
    let shown = vcx.read(|cx| view.read(cx).grid().visible_text());
    assert_eq!(shown[rows - 2], "line 196 of the log");

    // "line 194" on screen line 1 now: select "194 of" and copy it.
    // A press on a cell's left half starts the selection with that cell.
    let from = vcx
        .read(|cx| view.read(cx).cell_center(rows - 4, 5))
        .unwrap()
        - point(px(2.), px(0.));
    let to = vcx
        .read(|cx| view.read(cx).cell_center(rows - 4, 10))
        .unwrap();
    vcx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::none());
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    // The platform's own copy key: Cmd on macOS, Ctrl elsewhere.
    vcx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    });
    let copied = vcx.read_from_clipboard().and_then(|c| c.text());
    assert_eq!(copied.as_deref(), Some("194 of"));
}

#[gpui::test]
fn a_log_rewraps_to_the_pane_and_keeps_its_top(cx: &mut TestAppContext) {
    let (view, mut vcx) = window(cx, Sizing::Fit);
    let long = "word ".repeat(40);
    let text = format!("first line\r\n{long}\r\nshort\r\n{long}\r\nlast\r\n");
    replay(&view, &mut vcx, text.as_bytes(), &[4096]);
    for width in [700., 400., 900.] {
        vcx.simulate_resize(size(px(width), px(500.)));
        vcx.run_until_parked();
        let shown = vcx.read(|cx| view.read(cx).grid().visible_text());
        assert_eq!(shown[0], "first line", "at {width}px: {shown:?}");
        let all = vcx.read(|cx| view.read(cx).grid().all_text());
        assert!(
            all.contains(long.trim_end()) && all.contains("last"),
            "{all}"
        );
    }
}
