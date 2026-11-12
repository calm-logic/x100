//! The colours a cell names, resolved: the 16 named ones from a palette that sits on the app's
//! dark background, the 256-colour cube and grey ramp by their standard formula, and true colour
//! as given. A colour the program set with OSC 4, 10 or 11 wins over the palette.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui::{Hsla, Rgba};

pub const BACKGROUND: u32 = 0x0a0a0b;
pub const FOREGROUND: u32 = 0xd4d4d8;

const ANSI: [u32; 16] = [
    0x3a3a3d, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4, 0x6e7681,
    0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff,
];

pub fn hex(c: u32) -> Hsla {
    Rgba {
        r: ((c >> 16) & 0xff) as f32 / 255.,
        g: ((c >> 8) & 0xff) as f32 / 255.,
        b: (c & 0xff) as f32 / 255.,
        a: 1.,
    }
    .into()
}

fn rgb(c: Rgb) -> Hsla {
    hex((u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b))
}

/// One of the 256 indexed colours.
pub fn indexed(i: u8) -> u32 {
    match i {
        0..16 => ANSI[i as usize],
        16..232 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + u32::from(v) * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        232.. => {
            let v = 8 + (u32::from(i) - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}

pub fn resolve(color: Color, colors: &Colors) -> Hsla {
    match color {
        Color::Spec(c) => rgb(c),
        Color::Indexed(i) => colors[i as usize].map_or_else(|| hex(indexed(i)), rgb),
        Color::Named(n) => {
            if let Some(c) = colors[n] {
                return rgb(c);
            }
            let i = n as usize;
            match n {
                _ if i < 16 => hex(ANSI[i]),
                NamedColor::Background => hex(BACKGROUND),
                NamedColor::DimForeground => dim(hex(FOREGROUND)),
                _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                    .contains(&i) =>
                {
                    dim(hex(ANSI[i - NamedColor::DimBlack as usize]))
                }
                _ => hex(FOREGROUND),
            }
        }
    }
}

/// Faint text: the same hue, closer to the background.
pub fn dim(mut c: Hsla) -> Hsla {
    c.l *= 0.66;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_and_ramp_follow_the_standard_formula() {
        assert_eq!(indexed(16), 0x000000);
        assert_eq!(indexed(196), 0xff0000);
        assert_eq!(indexed(231), 0xffffff);
        assert_eq!(indexed(232), 0x080808);
        assert_eq!(indexed(255), 0xeeeeee);
    }
}
