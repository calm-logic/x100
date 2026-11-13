//! A line editor for the message box and the prompts.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default)]
pub struct Input {
    text: String,
    /// In characters, not bytes.
    cursor: usize,
}

impl Input {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Hands back the text and starts again empty.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.take();
    }

    fn byte(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert(c);
        }
    }

    /// Handles an editing key; false for a key that is not one.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                let to = self.byte(self.cursor);
                self.text.drain(..to);
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let before: Vec<char> = self.text.chars().take(self.cursor).collect();
                let mut start = before.len();
                while start > 0 && before[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !before[start - 1].is_whitespace() {
                    start -= 1;
                }
                let (a, b) = (self.byte(start), self.byte(self.cursor));
                self.text.drain(a..b);
                self.cursor = start;
            }
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.chars().count(),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(c)
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => self.insert('\n'),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Delete if self.cursor < self.text.chars().count() => {
                let at = self.byte(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.chars().count(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut Input, code: KeyCode) {
        input.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn edits_in_the_middle_of_wide_characters() {
        let mut i = Input::default();
        for c in "héllo".chars() {
            press(&mut i, KeyCode::Char(c));
        }
        press(&mut i, KeyCode::Left);
        press(&mut i, KeyCode::Left);
        press(&mut i, KeyCode::Backspace);
        assert_eq!(i.text(), "hélo");
        assert_eq!(i.cursor(), 2);
        press(&mut i, KeyCode::Backspace);
        assert_eq!(i.text(), "hlo");
    }
}
