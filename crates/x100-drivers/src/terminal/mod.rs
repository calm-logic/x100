//! A driver's terminal: the emulator that keeps its screen and answers the queries its program
//! makes, and the state reports the program writes to it.
//!
//! The daemon records a terminal as [`Chunk`]s: every byte the program wrote and every resize, the
//! starting size first. [`Emulator::replay`] turns a recording back into a screen, which is how a
//! client draws one and how tests check that a recording is complete.

pub(crate) mod pty;

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::vte::ansi::{Processor, Rgb};
use base64::Engine;
use x100_core::terminal::{Chunk, ChunkKind};

/// A terminal's size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

/// The size a terminal starts at, before any viewer has said what its pane holds: what the app's
/// terminal pane holds whole at its usual 1400 by 900 window, so a driver nobody watched while it
/// ran is still readable at the app's normal font.
impl Default for Size {
    fn default() -> Self {
        Self {
            cols: 100,
            rows: 40,
        }
    }
}

/// A cell's assumed size in pixels, for programs that ask for the window's size in pixels.
const CELL: (u16, u16) = (8, 16);

/// Collects what the terminal must write back to the program while bytes are fed to it.
#[derive(Default, Clone)]
struct Answers(Arc<Mutex<Vec<u8>>>);

impl EventListener for Answers {
    fn send_event(&self, event: Event) {
        let answer = match event {
            Event::PtyWrite(text) => text,
            Event::ColorRequest(index, format) => format(palette(index)),
            Event::TextAreaSizeRequest(format) => format(WindowSize {
                num_lines: 0,
                num_cols: 0,
                cell_width: CELL.0,
                cell_height: CELL.1,
            }),
            _ => return,
        };
        self.0
            .lock()
            .expect("answers")
            .extend_from_slice(answer.as_bytes());
    }
}

/// The colours a program is told when it asks: a dark theme, which matches the theme x100 seeds
/// in Claude Code's configuration, and the xterm palette.
fn palette(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    let rgb = |(r, g, b): (u8, u8, u8)| Rgb { r, g, b };
    match index {
        0..=15 => rgb(ANSI[index]),
        16..=231 => {
            let n = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            rgb((level(n / 36), level(n / 6 % 6), level(n % 6)))
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            rgb((v, v, v))
        }
        // Alacritty numbers the background after the 256 indexed colours and the foreground.
        257 => rgb((24, 24, 24)),
        _ => rgb((229, 229, 229)),
    }
}

/// A terminal emulator fed a program's output. It answers the program's queries (device
/// attributes, cursor position, colours, modes) and holds the screen.
pub struct Emulator {
    term: Term<Answers>,
    answers: Answers,
    parser: Processor,
    size: Size,
}

impl Emulator {
    pub fn new(size: Size) -> Self {
        let config = term::Config {
            // Without it the program sends plain keys, which is all x100 ever types.
            kitty_keyboard: false,
            ..Default::default()
        };
        let dims = TermSize::new(usize::from(size.cols), usize::from(size.rows));
        let answers = Answers::default();
        Self {
            term: Term::new(config, &dims, answers.clone()),
            answers,
            parser: Processor::new(),
            size,
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// Feeds the program's output and returns what the terminal answers, to be written back to
    /// the program.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, bytes);
        std::mem::take(&mut *self.answers.0.lock().expect("answers"))
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        self.term.resize(TermSize::new(
            usize::from(size.cols),
            usize::from(size.rows),
        ));
    }

    pub fn apply(&mut self, chunk: &Chunk) {
        match &chunk.what {
            ChunkKind::Output { bytes } => {
                self.feed(&bytes.0);
            }
            ChunkKind::Resize { cols, rows } => self.resize(Size {
                cols: *cols,
                rows: *rows,
            }),
        }
    }

    /// The screen a recording ends on, or `None` if it does not start with its size.
    pub fn replay<'a>(chunks: impl IntoIterator<Item = &'a Chunk>) -> Option<Self> {
        let mut chunks = chunks.into_iter();
        let ChunkKind::Resize { cols, rows } = chunks.next()?.what else {
            return None;
        };
        let mut emulator = Self::new(Size { cols, rows });
        for chunk in chunks {
            emulator.apply(chunk);
        }
        Some(emulator)
    }

    /// The visible screen as text: one line per row, trailing spaces and blank rows removed.
    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let mut lines = Vec::with_capacity(grid.screen_lines());
        for row in 0..grid.screen_lines() {
            let line = &grid[Line(row as i32)];
            let mut text = String::new();
            for col in 0..grid.columns() {
                let cell = &line[Column(col)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                text.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    text.extend(extra);
                }
            }
            lines.push(text.trim_end().to_owned());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

/// A state a program reported on its terminal with OSC 7501, the program status protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reported {
    /// The program asked whether the terminal understands the protocol.
    Probe,
    Status {
        state: Status,
        /// The message the program attached, decoded.
        message: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Idle,
    Working,
    Done,
    Error,
    /// Waiting on a person: `permission`, `question` or `auth`.
    Blocked(String),
    Clear,
}

/// The answer to [`Reported::Probe`]: the states and kinds x100 understands.
pub(crate) const PROBE_ANSWER: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

fn parse_report(payload: &[u8]) -> Option<Reported> {
    let payload = std::str::from_utf8(payload).ok()?;
    if payload == "?" {
        return Some(Reported::Probe);
    }
    let mut state = None;
    let mut kind = None;
    let mut message = None;
    for field in payload.split(':') {
        let (key, value) = field.split_once('=')?;
        match key {
            "state" => state = Some(value),
            "kind" => kind = Some(value.to_owned()),
            "msg" => {
                message = base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok());
            }
            _ => {}
        }
    }
    let state = match state? {
        "idle" => Status::Idle,
        "working" => Status::Working,
        "done" => Status::Done,
        "error" => Status::Error,
        "blocked" => Status::Blocked(kind.unwrap_or_default()),
        "clear" => Status::Clear,
        _ => return None,
    };
    Some(Reported::Status { state, message })
}

/// Finds OSC 7501 reports in a byte stream that may split them anywhere.
#[derive(Default)]
pub(crate) struct ReportScanner {
    state: Scan,
}

#[derive(Default)]
enum Scan {
    #[default]
    Ground,
    Escape,
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
}

/// An OSC longer than this is not one x100 reads; it is dropped rather than buffered.
const OSC_LIMIT: usize = 64 * 1024;

impl ReportScanner {
    /// The reports that end in `bytes`, each with the offset just past its end.
    pub fn scan(&mut self, bytes: &[u8]) -> Vec<(usize, Reported)> {
        let mut found = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            self.state = match (std::mem::take(&mut self.state), b) {
                (Scan::Ground, 0x1b) => Scan::Escape,
                (Scan::Ground, _) => Scan::Ground,
                (Scan::Escape, b']') => Scan::Osc(Vec::new()),
                (Scan::Escape, 0x1b) => Scan::Escape,
                (Scan::Escape, _) => Scan::Ground,
                (Scan::Osc(body), 0x07) => {
                    found.extend(Self::finish(&body).map(|r| (i + 1, r)));
                    Scan::Ground
                }
                (Scan::Osc(body), 0x1b) => Scan::OscEscape(body),
                (Scan::Osc(mut body), b) => {
                    if body.len() < OSC_LIMIT {
                        body.push(b);
                        Scan::Osc(body)
                    } else {
                        Scan::Ground
                    }
                }
                (Scan::OscEscape(body), b'\\') => {
                    found.extend(Self::finish(&body).map(|r| (i + 1, r)));
                    Scan::Ground
                }
                // An escape that does not end the OSC starts a new sequence.
                (Scan::OscEscape(_), b']') => Scan::Osc(Vec::new()),
                (Scan::OscEscape(_), _) => Scan::Ground,
            };
        }
        found
    }

    fn finish(body: &[u8]) -> Option<Reported> {
        parse_report(body.strip_prefix(b"7501;")?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_found_across_any_split() {
        let stream = b"hi\x1b]0;title\x07\x1b]7501;?\x1b\\x\x1b]7501;state=blocked:app=claude-code:kind=auth:msg=UGxlYXNlIHJ1biAvbG9naW4=\x1b\\\x1b]7501;state=done:app=codex\x07";
        let expected = vec![
            Reported::Probe,
            Reported::Status {
                state: Status::Blocked("auth".into()),
                message: Some("Please run /login".into()),
            },
            Reported::Status {
                state: Status::Done,
                message: None,
            },
        ];
        for split in 0..stream.len() {
            let mut scanner = ReportScanner::default();
            let mut found: Vec<_> = scanner
                .scan(&stream[..split])
                .into_iter()
                .map(|f| f.1)
                .collect();
            found.extend(scanner.scan(&stream[split..]).into_iter().map(|f| f.1));
            assert_eq!(found, expected, "split at {split}");
        }
    }

    #[test]
    fn the_emulator_answers_queries_and_keeps_the_screen() {
        let mut emulator = Emulator::new(Size { cols: 20, rows: 4 });
        let answers = emulator.feed(b"hello\r\n\x1b[6n\x1b[c\x1b]11;?\x1b\\");
        let answers = String::from_utf8(answers).unwrap();
        assert!(answers.contains("\x1b[2;1R"), "{answers:?}");
        assert!(answers.contains("\x1b[?6c"), "{answers:?}");
        assert!(answers.contains("\x1b]11;rgb:"), "{answers:?}");
        emulator.feed(b"world");
        assert_eq!(emulator.text(), "hello\nworld");
        emulator.resize(Size { cols: 10, rows: 2 });
        assert_eq!(emulator.size(), Size { cols: 10, rows: 2 });
    }
}
