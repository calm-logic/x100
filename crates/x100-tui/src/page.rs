//! What a tab shows: lines, some of which can be selected. A tab builds its page from the state
//! the client holds each time it is drawn, so what is on screen is that state and nothing older.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use x100_core::id::{ActionId, AgentId, CredentialRef, DriverId, FileId, TaskId};

pub const DIM: Style = Style::new().fg(Color::DarkGray);
pub const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
pub const ACCENT: Style = Style::new().fg(Color::Cyan);
pub const GOOD: Style = Style::new().fg(Color::Green);
pub const WARN: Style = Style::new().fg(Color::Yellow);
pub const BAD: Style = Style::new().fg(Color::Red);

/// What a selectable line stands for, and so what a key acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Agent(AgentId),
    Driver(DriverId),
    Task(TaskId),
    File(FileId),
    Action(ActionId),
    Lead(AgentId),
    Setting(Setting),
    Credential(CredentialRef),
    Skill(x100_core::memory::SkillRef),
}

/// A control in Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    AskMeAbout,
    Budget,
    LabShare,
    TrialBeforeCommit,
    Allow(x100_core::harness::HarnessModel),
    DecisionModel,
    ReflectNow,
    HomeboxLocal,
    HomeboxAddress,
    GitHub(x100_proto::GitHubMethod),
    Disconnect(x100_proto::Integration),
    /// Link the project to the Homebox project of this repository, `owner/name`.
    LinkHomebox(String),
    UnlinkHomebox,
    Deploy,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub line: Line<'static>,
    pub target: Option<Target>,
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub items: Vec<Item>,
}

impl Page {
    pub fn new() -> Page {
        Page::default()
    }

    pub fn line(&mut self, line: impl Into<Line<'static>>) -> &mut Self {
        self.items.push(Item {
            line: line.into(),
            target: None,
        });
        self
    }

    pub fn text(&mut self, text: impl Into<String>) -> &mut Self {
        self.line(Line::raw(text.into()))
    }

    pub fn styled(&mut self, text: impl Into<String>, style: Style) -> &mut Self {
        self.line(Line::styled(text.into(), style))
    }

    pub fn blank(&mut self) -> &mut Self {
        self.text("")
    }

    pub fn heading(&mut self, text: &str) -> &mut Self {
        self.styled(text.to_uppercase(), DIM)
    }

    pub fn row(&mut self, line: impl Into<Line<'static>>, target: Target) -> &mut Self {
        self.items.push(Item {
            line: line.into(),
            target: Some(target),
        });
        self
    }

    /// Words wrapped to `width`, each line indented by `indent`.
    pub fn paragraph(&mut self, text: &str, indent: &str, width: usize) -> &mut Self {
        for l in wrap(text, width.saturating_sub(indent.chars().count()).max(8)) {
            self.text(format!("{indent}{l}"));
        }
        self
    }

    pub fn targets(&self) -> Vec<(usize, &Target)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| it.target.as_ref().map(|t| (i, t)))
            .collect()
    }

    /// The plain text of every line, for a test or a script.
    pub fn plain(&self) -> String {
        self.items
            .iter()
            .map(|i| {
                i.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A line of spans.
pub fn spans(parts: Vec<Span<'static>>) -> Line<'static> {
    Line::from(parts)
}

pub fn span(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// Words wrapped to `width` columns; a line break in the text stays one.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.replace('\r', "").split('\n') {
        let mut line = String::new();
        let mut len = 0;
        for word in raw.split(' ') {
            let mut word = word.to_owned();
            let mut w = word.chars().count();
            while w > width {
                if len > 0 {
                    out.push(std::mem::take(&mut line));
                    len = 0;
                }
                let head: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                w -= width;
                out.push(head);
            }
            if len > 0 && len + 1 + w > width {
                out.push(std::mem::take(&mut line));
                len = 0;
            }
            if len > 0 {
                line.push(' ');
                len += 1;
            }
            line.push_str(&word);
            len += w;
        }
        out.push(line);
    }
    out
}

/// Cuts `s` to `width` columns, ending in an ellipsis if it was longer.
pub fn clip(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(width.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// `s` padded or cut to exactly `width` columns.
pub fn cell(s: &str, width: usize) -> String {
    let c = clip(s, width);
    let pad = width.saturating_sub(c.chars().count());
    format!("{c}{}", " ".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_words_and_breaks_long_ones() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("a\nb", 9), ["a", "b"]);
    }
}
