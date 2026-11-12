//! A live terminal: alacritty_terminal parses the PTY output into a grid on its own thread, and
//! this view draws the grid as one line of styled text per row. Zed's own terminal crates are
//! GPL, so none of their code is used.

use std::{collections::HashMap, sync::Arc};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::Scroll,
    index::Point as GridPoint,
    sync::FairMutex,
    term::{Config, Term, cell::Flags, color::Colors, test::TermSize},
    tty,
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, FocusHandle, Font, FontStyle, FontWeight, Hsla, IntoElement, KeyDownEvent,
    ParentElement, Pixels, Render, ScrollEvent, Styled, StyledText, TextRun, Window, div, font,
    prelude::*, px, rgb,
};

use crate::stats;

const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f32 = 17.;
const DEFAULT_FG: u32 = 0xd1d7e0;
const DEFAULT_BG: u32 = 0x0d1117;

#[derive(Clone)]
struct Listener(mpsc::UnboundedSender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.unbounded_send(event);
    }
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: (usize, usize),
    cell_width: Option<Pixels>,
    scroll_accum: f32,
    exited: Option<String>,
    focus: FocusHandle,
    title: String,
    fonts: [Font; 3],
}

impl TerminalView {
    pub fn new(program: String, args: Vec<String>, cx: &mut Context<Self>) -> Self {
        let (cols, lines) = (80usize, 24usize);
        let (tx, mut rx) = mpsc::unbounded();
        let listener = Listener(tx);
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &TermSize::new(cols, lines),
            listener.clone(),
        )));
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        let opts = tty::Options {
            shell: Some(tty::Shell::new(program.clone(), args.clone())),
            working_directory: None,
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&opts, window_size(cols, lines, px(8.)), 0).expect("open pty");
        let event_loop =
            EventLoop::new(term.clone(), listener, pty, true, false).expect("event loop");
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        // Wakeups arrive once per PTY read. Draining the queue before notifying coalesces a
        // burst of output into one render.
        cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut events = vec![first];
                while let Ok(e) = rx.try_recv() {
                    events.push(e);
                }
                let r = this.update(cx, |view, cx| {
                    for event in events {
                        match event {
                            Event::Wakeup => stats::bump(&stats::TERM_WAKEUPS),
                            Event::PtyWrite(text) => view.write(text.into_bytes()),
                            Event::Title(t) => view.title = t,
                            Event::ChildExit(status) => view.exited = Some(format!("{status}")),
                            Event::Exit if view.exited.is_none() => {
                                view.exited = Some("exit".into());
                            }
                            _ => {}
                        }
                    }
                    cx.notify();
                });
                if r.is_err() {
                    break;
                }
            }
        })
        .detach();

        let base = font("DejaVu Sans Mono");
        let bold = Font {
            weight: FontWeight::BOLD,
            ..base.clone()
        };
        let italic = Font {
            style: FontStyle::Italic,
            ..base.clone()
        };
        Self {
            term,
            notifier,
            size: (cols, lines),
            cell_width: None,
            scroll_accum: 0.,
            exited: None,
            focus: cx.focus_handle(),
            title: format!("{program} {}", args.join(" ")),
            fonts: [base, bold, italic],
        }
    }

    fn write(&self, bytes: Vec<u8>) {
        let _ = self.notifier.0.send(Msg::Input(bytes.into()));
    }

    /// Fits the grid and the PTY to the space the pane has, so a resized window reflows the
    /// child program the way a real terminal does.
    fn fit(&mut self, width: Pixels, height: Pixels, window: &mut Window) {
        let cell_width = *self.cell_width.get_or_insert_with(|| {
            let ts = window.text_system();
            let id = ts.resolve_font(&self.fonts[0]);
            ts.advance(id, px(FONT_SIZE), 'm')
                .map(|s| s.width)
                .unwrap_or(px(8.))
        });
        let cols = ((width / cell_width).floor() as usize).max(10);
        let lines = ((height / px(LINE_HEIGHT)).floor() as usize).max(4);
        if (cols, lines) != self.size {
            self.size = (cols, lines);
            self.term.lock().resize(TermSize::new(cols, lines));
            let _ = self
                .notifier
                .0
                .send(Msg::Resize(window_size(cols, lines, cell_width)));
        }
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _w: &mut Window, cx: &mut Context<Self>) {
        self.scroll_accum += f32::from(event.delta.pixel_delta(px(LINE_HEIGHT)).y) / LINE_HEIGHT;
        let lines = self.scroll_accum.trunc() as i32;
        if lines != 0 {
            self.scroll_accum -= lines as f32;
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let bytes: Option<Vec<u8>> = match ks.key.as_str() {
            "enter" => Some(b"\r".to_vec()),
            "backspace" => Some(vec![0x7f]),
            "tab" => Some(b"\t".to_vec()),
            "escape" => Some(vec![0x1b]),
            "up" => Some(b"\x1b[A".to_vec()),
            "down" => Some(b"\x1b[B".to_vec()),
            "right" => Some(b"\x1b[C".to_vec()),
            "left" => Some(b"\x1b[D".to_vec()),
            k if ks.modifiers.control && k.len() == 1 => {
                let c = k.as_bytes()[0].to_ascii_lowercase();
                c.is_ascii_lowercase().then(|| vec![c - b'a' + 1])
            }
            _ => ks.key_char.clone().map(String::into_bytes),
        };
        if let Some(bytes) = bytes {
            self.term.lock().scroll_display(Scroll::Bottom);
            self.write(bytes);
            cx.stop_propagation();
        }
    }
}

fn window_size(cols: usize, lines: usize, cell_width: Pixels) -> WindowSize {
    WindowSize {
        num_lines: lines as u16,
        num_cols: cols as u16,
        cell_width: f32::from(cell_width) as u16,
        cell_height: LINE_HEIGHT as u16,
    }
}

fn hex(c: u32) -> Hsla {
    rgb(c).into()
}

fn from_rgb(c: Rgb) -> Hsla {
    hex(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32)
}

const ANSI: [u32; 16] = [
    0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4, 0x6e7681,
    0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
];

fn indexed(i: u8) -> Hsla {
    match i {
        0..16 => hex(ANSI[i as usize]),
        16..232 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            hex((level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6))
        }
        _ => {
            let v = 8 + (i as u32 - 232) * 10;
            hex((v << 16) | (v << 8) | v)
        }
    }
}

fn resolve(color: Color, colors: &Colors, fg: bool) -> Hsla {
    match color {
        Color::Spec(c) => from_rgb(c),
        Color::Indexed(i) => colors[i as usize]
            .map(from_rgb)
            .unwrap_or_else(|| indexed(i)),
        Color::Named(n) => {
            if let Some(c) = colors[n] {
                return from_rgb(c);
            }
            let i = n as usize;
            match n {
                _ if i < 16 => hex(ANSI[i]),
                NamedColor::Background => hex(DEFAULT_BG),
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => {
                    hex(DEFAULT_FG)
                }
                NamedColor::DimForeground => hex(0x9198a1),
                _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                    .contains(&i) =>
                {
                    let mut c = hex(ANSI[i - NamedColor::DimBlack as usize]);
                    c.l *= 0.7;
                    c
                }
                _ => hex(if fg { DEFAULT_FG } else { DEFAULT_BG }),
            }
        }
    }
}

struct Line {
    text: String,
    runs: Vec<TextRun>,
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        stats::bump(&stats::TERM_RENDERS);
        // The terminal pane is whatever the fixed-width list and Markdown columns leave.
        let vp = window.viewport_size();
        self.fit(
            vp.width - px(crate::LIST_WIDTH + crate::MARKDOWN_WIDTH + 24.),
            vp.height - px(crate::HEADER_HEIGHT + 40.),
            window,
        );

        let lines = {
            let term = self.term.lock();
            let content = term.renderable_content();
            let cursor = content.cursor;
            let show_cursor = cursor.shape != CursorShape::Hidden;
            let colors = content.colors;
            let mut lines: Vec<Line> = Vec::with_capacity(self.size.1);
            let mut current_line = None;
            for indexed in content.display_iter {
                let GridPoint { line, column } = indexed.point;
                if current_line != Some(line) {
                    current_line = Some(line);
                    lines.push(Line {
                        text: String::new(),
                        runs: Vec::new(),
                    });
                }
                let cell = &indexed.cell;
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let mut fg = resolve(cell.fg, colors, true);
                let mut bg = resolve(cell.bg, colors, false);
                let at_cursor =
                    show_cursor && line == cursor.point.line && column == cursor.point.column;
                if cell.flags.contains(Flags::INVERSE) ^ at_cursor {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.flags.contains(Flags::DIM) {
                    fg.a *= 0.7;
                }
                let font = if cell.flags.contains(Flags::BOLD) {
                    &self.fonts[1]
                } else if cell.flags.contains(Flags::ITALIC) {
                    &self.fonts[2]
                } else {
                    &self.fonts[0]
                };
                let background = (bg != hex(DEFAULT_BG)).then_some(bg);
                let l = lines.last_mut().unwrap();
                let start = l.text.len();
                l.text.push(if cell.c == '\0' { ' ' } else { cell.c });
                let len = l.text.len() - start;
                match l.runs.last_mut() {
                    Some(r)
                        if r.color == fg && r.background_color == background && r.font == *font =>
                    {
                        r.len += len
                    }
                    _ => l.runs.push(TextRun {
                        len,
                        font: font.clone(),
                        color: fg,
                        background_color: background,
                        underline: None,
                        strikethrough: None,
                    }),
                }
            }
            lines
        };

        let status = match &self.exited {
            Some(code) => format!("{}  [exited: {code}]", self.title),
            None => format!("{}  {}x{}", self.title, self.size.0, self.size.1),
        };
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_click(cx.listener(|this, _, window, cx| this.focus.focus(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(DEFAULT_BG))
            .font_family("DejaVu Sans Mono")
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .child(
                div()
                    .px_2()
                    .text_color(rgb(0x6e7681))
                    .bg(rgb(0x161b22))
                    .child(status),
            )
            .child(
                div()
                    .p_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .children(lines.into_iter().map(|l| {
                        div()
                            .h(px(LINE_HEIGHT))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(StyledText::new(l.text).with_runs(l.runs))
                    })),
            )
    }
}
