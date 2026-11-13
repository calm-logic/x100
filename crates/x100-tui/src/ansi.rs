//! The SGR text the shared output writer produces, as styled lines. It uses a handful of codes:
//! reset, bold, dim and the basic colours. Anything else in the text is dropped.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Lines grown by feeding text, the way a terminal grows its scrollback. The last line is the one
/// being written.
#[derive(Debug, Clone, Default)]
pub struct AnsiLog {
    lines: Vec<Vec<(Style, String)>>,
    style: Style,
}

impl AnsiLog {
    pub fn feed(&mut self, text: &str) {
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        let mut chars = text.chars().peekable();
        let mut run = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\u{1b}' => {
                    self.flush(&mut run);
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        let mut code = String::new();
                        for c in chars.by_ref() {
                            if c.is_ascii_alphabetic() {
                                if c == 'm' {
                                    self.sgr(&code);
                                }
                                break;
                            }
                            code.push(c);
                        }
                    }
                }
                '\r' => {}
                '\n' => {
                    self.flush(&mut run);
                    self.lines.push(Vec::new());
                }
                c => run.push(c),
            }
        }
        self.flush(&mut run);
    }

    fn flush(&mut self, run: &mut String) {
        if !run.is_empty() {
            let style = self.style;
            if let Some(line) = self.lines.last_mut() {
                line.push((style, std::mem::take(run)));
            }
        }
    }

    fn sgr(&mut self, code: &str) {
        for part in code.split(';') {
            self.style = match part {
                "" | "0" => Style::default(),
                "1" => self.style.add_modifier(Modifier::BOLD),
                "2" => self.style.add_modifier(Modifier::DIM),
                "31" => self.style.fg(Color::Red),
                "32" => self.style.fg(Color::Green),
                "33" => self.style.fg(Color::Yellow),
                "36" => self.style.fg(Color::Cyan),
                _ => self.style,
            };
        }
    }

    pub fn lines(&self) -> impl Iterator<Item = Line<'static>> + '_ {
        self.lines.iter().map(|spans| {
            Line::from(
                spans
                    .iter()
                    .map(|(style, text)| Span::styled(text.clone(), *style))
                    .collect::<Vec<_>>(),
            )
        })
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The text without styling, one line per line.
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().map(|(_, t)| t.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_lines_come_through() {
        let mut log = AnsiLog::default();
        log.feed("\u{1b}[36m▸ write\u{1b}[0m x\r\n  \u{1b}[32m✓\u{1b}[0m ok\r\n");
        log.feed("tail");
        assert_eq!(log.text(), "▸ write x\n  ✓ ok\ntail");
        let first = log.lines().next().unwrap();
        assert_eq!(first.spans[0].style.fg, Some(Color::Cyan));
    }
}
