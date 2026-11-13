//! An agent's look in a terminal: its outline as one glyph, drawn in its colour where the terminal
//! shows colour. The desktop app draws the same two choices as a face.

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use x100_core::appearance::{Appearance, Hue, Outline};

/// One column wide in every terminal, so the tree's columns stay aligned.
pub fn glyph(o: Outline) -> &'static str {
    match o {
        Outline::Circle => "●",
        Outline::Square => "■",
        Outline::Hexagon => "⬢",
        Outline::Diamond => "◆",
        Outline::Octagon => "⯄",
        Outline::Triangle => "▲",
        Outline::Shield => "⛊",
        Outline::Capsule => "⬬",
    }
}

/// How much colour the terminal shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colours {
    /// `NO_COLOR` is set: every agent is drawn in the text colour.
    None,
    /// The sixteen named colours every terminal has.
    Basic,
    /// 24-bit colour, as `COLORTERM` says: the app's own shades.
    True,
}

impl Colours {
    pub fn from_env() -> Colours {
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Colours::None;
        }
        match std::env::var("COLORTERM").as_deref() {
            Ok("truecolor" | "24bit") => Colours::True,
            _ => Colours::Basic,
        }
    }

    /// The terminal colour for `hue`; `None` draws in the text colour.
    pub fn of(self, hue: Option<Hue>) -> Option<Color> {
        let hue = hue?;
        match self {
            Colours::None => None,
            // The same shades as the app's theme.
            Colours::True => Some(match hue {
                Hue::Red => Color::Rgb(0xff, 0x6b, 0x6b),
                Hue::Orange => Color::Rgb(0xff, 0x9f, 0x45),
                Hue::Yellow => Color::Rgb(0xf5, 0xd1, 0x50),
                Hue::Green => Color::Rgb(0x5a, 0xd4, 0x8a),
                Hue::Teal => Color::Rgb(0x3f, 0xd0, 0xc9),
                Hue::Blue => Color::Rgb(0x5a, 0xa9, 0xff),
                Hue::Violet => Color::Rgb(0xa7, 0x8b, 0xfa),
                Hue::Pink => Color::Rgb(0xf4, 0x72, 0xb6),
            }),
            // Orange has no name of its own; light red is the nearest that stays apart from red
            // and yellow.
            Colours::Basic => Some(match hue {
                Hue::Red => Color::Red,
                Hue::Orange => Color::LightRed,
                Hue::Yellow => Color::Yellow,
                Hue::Green => Color::Green,
                Hue::Teal => Color::Cyan,
                Hue::Blue => Color::Blue,
                Hue::Violet => Color::Magenta,
                Hue::Pink => Color::LightMagenta,
            }),
        }
    }
}

/// The agent's face as a span: its outline's glyph in its colour.
pub fn face(look: Appearance, colours: Colours) -> Span<'static> {
    let style = colours
        .of(look.hue)
        .map_or_else(Style::new, |c| Style::new().fg(c));
    Span::styled(glyph(look.outline), style)
}

/// "1 Circle  2 Square  …", the choices a prompt offers, numbered from 1.
pub fn outline_choices() -> String {
    Outline::ALL
        .iter()
        .enumerate()
        .map(|(i, o)| {
            format!(
                "{} {} {}",
                i + 1,
                glyph(*o),
                x100_client::words::outline(*o)
            )
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// "0 No color  1 Red  …".
pub fn hue_choices() -> String {
    std::iter::once(None)
        .chain(Hue::ALL.into_iter().map(Some))
        .enumerate()
        .map(|(i, h)| format!("{i} {}", x100_client::words::hue(h)))
        .collect::<Vec<_>>()
        .join("  ")
}

/// The outline a prompt's answer names: its number from 1, or its name.
pub fn parse_outline(text: &str) -> Option<Outline> {
    let text = text.trim();
    if let Ok(n) = text.parse::<usize>() {
        return Outline::ALL.get(n.checked_sub(1)?).copied();
    }
    Outline::ALL
        .into_iter()
        .find(|o| x100_client::words::outline(*o).eq_ignore_ascii_case(text))
}

/// The colour a prompt's answer names: 0 or "none" for no colour, its number from 1, or its name.
/// The outer `None` is an answer that names nothing.
pub fn parse_hue(text: &str) -> Option<Option<Hue>> {
    let text = text.trim();
    if let Ok(n) = text.parse::<usize>() {
        return match n {
            0 => Some(None),
            n => Hue::ALL.get(n - 1).map(|h| Some(*h)),
        };
    }
    if text.eq_ignore_ascii_case("none") || text.eq_ignore_ascii_case("no color") {
        return Some(None);
    }
    Hue::ALL
        .into_iter()
        .find(|h| x100_client::words::hue(Some(*h)).eq_ignore_ascii_case(text))
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_one_column_and_every_choice_parses_back() {
        for (i, o) in Outline::ALL.into_iter().enumerate() {
            assert_eq!(Span::raw(glyph(o)).width(), 1, "{o:?}");
            assert_eq!(parse_outline(&(i + 1).to_string()), Some(o));
            assert_eq!(parse_outline(x100_client::words::outline(o)), Some(o));
        }
        assert_eq!(parse_outline("9"), None);
        assert_eq!(parse_outline("0"), None);
        for (i, h) in Hue::ALL.into_iter().enumerate() {
            assert_eq!(parse_hue(&(i + 1).to_string()), Some(Some(h)));
            assert_eq!(
                parse_hue(&x100_client::words::hue(Some(h)).to_lowercase()),
                Some(Some(h))
            );
        }
        assert_eq!(parse_hue("0"), Some(None));
        assert_eq!(parse_hue("none"), Some(None));
        assert_eq!(parse_hue("mauve"), None);
    }

    #[test]
    fn colour_follows_what_the_terminal_shows() {
        assert_eq!(Colours::None.of(Some(Hue::Teal)), None);
        assert_eq!(Colours::Basic.of(Some(Hue::Teal)), Some(Color::Cyan));
        assert!(matches!(
            Colours::True.of(Some(Hue::Teal)),
            Some(Color::Rgb(..))
        ));
        assert_eq!(Colours::True.of(None), None);
    }
}
