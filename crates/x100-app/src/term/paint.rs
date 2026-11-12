//! Drawing the grid. Each screen line is turned into what it paints once, when it changes:
//! background runs, then glyphs placed cell by cell at fixed positions, then underlines and
//! strikes. A frame paints those lines as they are, so a wakeup that changed one line builds one
//! line, and a wide character, an emoji or a box-drawing glyph from a fallback font cannot push
//! the rest of its line out of its columns.

use std::collections::HashMap;
use std::rc::Rc;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui::{
    Bounds, Font, FontId, FontStyle, FontWeight, GlyphId, Hsla, Pixels, TextRun, Window, fill,
    font, point, px, size,
};

use super::palette::{self, resolve};

/// A cell's font: bit 0 bold, bit 1 italic.
type Variant = u8;

/// The four faces of the terminal's font.
pub struct Fonts([Font; 4]);

impl Fonts {
    pub fn new(family: &'static str) -> Self {
        let base = font(family);
        let bold = Font {
            weight: FontWeight::BOLD,
            ..base.clone()
        };
        let italic = Font {
            style: FontStyle::Italic,
            ..base.clone()
        };
        let both = Font {
            style: FontStyle::Italic,
            ..bold.clone()
        };
        Self([base, bold, italic, both])
    }
}

/// The size of a cell and where in it the baseline sits, at one font size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub baseline: Pixels,
}

impl Metrics {
    pub fn measure(fonts: &Fonts, font_size: Pixels, window: &Window) -> Metrics {
        let text = window.text_system();
        let id = text.resolve_font(&fonts.0[0]);
        let width = text
            .advance(id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(px(0.));
        let cell_width = if width > px(0.) {
            width
        } else {
            font_size * 0.6
        };
        let ascent = text.ascent(id, font_size).abs();
        let descent = text.descent(id, font_size).abs();
        // The font's own height, so box-drawing lines meet from one row to the next.
        let natural = (ascent + descent).ceil();
        let line_height = if natural > px(0.) {
            natural
        } else {
            (font_size * 1.2).ceil()
        };
        let baseline = ((line_height - ascent - descent) / 2. + ascent).round();
        Metrics {
            font_size,
            cell_width,
            line_height,
            baseline,
        }
    }
}

#[derive(Clone, Copy)]
struct Glyph {
    font: FontId,
    id: GlyphId,
    x: Pixels,
    emoji: bool,
}

#[derive(Clone)]
struct Shaped {
    glyphs: Rc<[Glyph]>,
    width: Pixels,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Char(char, Variant),
    Cluster(String, Variant),
}

/// Each character shaped once per font size, fallback fonts included.
#[derive(Default)]
pub struct Shapes(HashMap<Key, Shaped>);

impl Shapes {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    fn get(&mut self, key: Key, fonts: &Fonts, font_size: Pixels, window: &Window) -> Shaped {
        if let Some(s) = self.0.get(&key) {
            return s.clone();
        }
        let (text, variant) = match &key {
            Key::Char(c, v) => (c.to_string(), *v),
            Key::Cluster(s, v) => (s.clone(), *v),
        };
        let run = TextRun {
            len: text.len(),
            font: fonts.0[usize::from(variant)].clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let layout = window
            .text_system()
            .layout_line(text.as_str(), font_size, &[run], None);
        let glyphs = layout
            .runs
            .iter()
            .flat_map(|r| {
                r.glyphs.iter().map(|g| Glyph {
                    font: r.font_id,
                    id: g.id,
                    x: g.position.x,
                    emoji: g.is_emoji,
                })
            })
            .collect();
        let shaped = Shaped {
            glyphs,
            width: layout.width,
        };
        self.0.insert(key, shaped.clone());
        shaped
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stroke {
    Under,
    DoubleUnder,
    Strike,
}

/// A run of cells on one line: its first column and how many.
#[derive(Clone, Copy)]
struct Span {
    col: u16,
    len: u16,
}

struct PlacedGlyph {
    col: u16,
    /// From the cell's left edge, to centre a glyph narrower than its cells.
    dx: Pixels,
    glyph: Glyph,
    color: Hsla,
}

/// One screen line, ready to paint.
#[derive(Default)]
pub struct RowPaint {
    backgrounds: Vec<(Span, Hsla)>,
    glyphs: Vec<PlacedGlyph>,
    strokes: Vec<(Span, Hsla, Stroke)>,
}

/// Builds screen line `row` as the grid holds it now.
pub fn build_row<T>(
    term: &Term<T>,
    row: usize,
    metrics: &Metrics,
    fonts: &Fonts,
    shapes: &mut Shapes,
    window: &Window,
) -> RowPaint {
    let colors = term.colors();
    let default_bg = resolve(Color::Named(NamedColor::Background), colors);
    let line = Line(row as i32 - term.grid().display_offset() as i32);
    let cells = &term.grid()[line];
    let mut out = RowPaint::default();
    for col in 0..term.columns() {
        let cell: &Cell = &cells[Column(col)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let len: u16 = if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let span = Span {
            col: col as u16,
            len,
        };
        let mut fg = resolve(cell.fg, colors);
        let mut bg = resolve(cell.bg, colors);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::DIM) {
            fg = palette::dim(fg);
        }
        if bg != default_bg {
            match out.backgrounds.last_mut() {
                Some((s, c)) if *c == bg && s.col + s.len == span.col => s.len += len,
                _ => out.backgrounds.push((span, bg)),
            }
        }
        let hidden = cell.flags.contains(Flags::HIDDEN);
        if !hidden && !matches!(cell.c, ' ' | '\0') {
            let variant = u8::from(cell.flags.contains(Flags::BOLD))
                | (u8::from(cell.flags.contains(Flags::ITALIC)) << 1);
            let key = match cell.zerowidth() {
                Some(extra) => {
                    let mut s = String::from(cell.c);
                    s.extend(extra);
                    Key::Cluster(s, variant)
                }
                None => Key::Char(cell.c, variant),
            };
            let shaped = shapes.get(key, fonts, metrics.font_size, window);
            let room = metrics.cell_width * f32::from(len);
            let dx = if shaped.width < room {
                ((room - shaped.width) / 2.).floor()
            } else {
                px(0.)
            };
            out.glyphs.extend(shaped.glyphs.iter().map(|g| PlacedGlyph {
                col: span.col,
                dx: dx + g.x,
                glyph: *g,
                color: fg,
            }));
        }
        let stroke_color = cell.underline_color().map_or(fg, |c| resolve(c, colors));
        let under = if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
            Some(Stroke::DoubleUnder)
        } else if cell.flags.intersects(Flags::ALL_UNDERLINES) {
            Some(Stroke::Under)
        } else {
            None
        };
        for (stroke, colour) in under.map(|u| (u, stroke_color)).into_iter().chain(
            cell.flags
                .contains(Flags::STRIKEOUT)
                .then_some((Stroke::Strike, fg)),
        ) {
            match out.strokes.last_mut() {
                Some((s, c, k)) if *c == colour && *k == stroke && s.col + s.len == span.col => {
                    s.len += len
                }
                _ => out.strokes.push((span, colour, stroke)),
            }
        }
    }
    out
}

/// Everything a frame paints, worked out before painting.
pub struct Frame {
    pub origin: gpui::Point<Pixels>,
    /// The whole pane, filled with the background.
    pub clip: Bounds<Pixels>,
    /// The part of it inside the padding, which cells are drawn in.
    pub content: Bounds<Pixels>,
    pub metrics: Metrics,
    pub rows: Vec<Rc<RowPaint>>,
    pub background: Hsla,
    pub cursor: Option<Cursor>,
    /// Per screen line, the selected columns.
    pub selected: Vec<(usize, Cols)>,
}

#[derive(Clone, Copy)]
pub struct Cols {
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Copy)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub wide: bool,
    pub shape: CursorShape,
    pub color: Hsla,
}

/// The cursor, when it is shown on screen: the scrollback hides it, and so does the program.
pub fn cursor<T: EventListener>(term: &Term<T>) -> Option<Cursor> {
    let content = term.renderable_content();
    let c = content.cursor;
    if c.shape == CursorShape::Hidden || content.display_offset != 0 {
        return None;
    }
    let row = usize::try_from(c.point.line.0).ok()?;
    let wide = term.grid()[c.point.line][c.point.column]
        .flags
        .contains(Flags::WIDE_CHAR);
    Some(Cursor {
        row,
        col: c.point.column.0,
        wide,
        shape: c.shape,
        color: resolve(Color::Named(NamedColor::Cursor), content.colors),
    })
}

/// The selected columns of each screen line.
pub fn selection<T>(term: &Term<T>, range: Option<SelectionRange>) -> Vec<(usize, Cols)> {
    let Some(range) = range else {
        return Vec::new();
    };
    let offset = term.grid().display_offset() as i32;
    let cols = term.columns();
    (0..term.screen_lines())
        .filter_map(|row| {
            let line = Line(row as i32 - offset);
            if line < range.start.line || line > range.end.line {
                return None;
            }
            let (from, to) = if range.is_block {
                (range.start.column.0, range.end.column.0)
            } else {
                let from = if line == range.start.line {
                    range.start.column.0
                } else {
                    0
                };
                let to = if line == range.end.line {
                    range.end.column.0
                } else {
                    cols - 1
                };
                (from, to)
            };
            (from <= to).then_some((row, Cols { from, to }))
        })
        .collect()
}

const SELECTION: Hsla = Hsla {
    h: 0.59,
    s: 0.9,
    l: 0.6,
    a: 0.35,
};

pub fn paint(frame: &Frame, window: &mut Window) {
    let m = frame.metrics;
    let cell = |row: usize, col: usize, cols: usize| Bounds {
        origin: point(
            frame.origin.x + m.cell_width * col as f32,
            frame.origin.y + m.line_height * row as f32,
        ),
        size: size(m.cell_width * cols as f32, m.line_height),
    };
    window.with_content_mask(Some(gpui::ContentMask { bounds: frame.clip }), |window| {
        window.paint_quad(fill(frame.clip, frame.background));
    });
    // A grid moved within a pane smaller than it shows only inside the padding.
    window.with_content_mask(
        Some(gpui::ContentMask {
            bounds: frame.content,
        }),
        |window| {
            for (i, row) in frame.rows.iter().enumerate() {
                for (span, color) in &row.backgrounds {
                    window.paint_quad(fill(
                        cell(i, usize::from(span.col), usize::from(span.len)),
                        *color,
                    ));
                }
            }
            for (row, span) in &frame.selected {
                window.paint_quad(fill(
                    cell(*row, span.from, span.to - span.from + 1),
                    SELECTION,
                ));
            }
            let block = frame
                .cursor
                .filter(|c| c.shape == CursorShape::Block)
                .map(|c| (c.row, c.col));
            if let Some(c) = frame.cursor.filter(|c| c.shape == CursorShape::Block) {
                window.paint_quad(fill(
                    cell(c.row, c.col, if c.wide { 2 } else { 1 }),
                    c.color,
                ));
            }
            for (i, row) in frame.rows.iter().enumerate() {
                let top = frame.origin.y + m.line_height * i as f32;
                for g in &row.glyphs {
                    let col = usize::from(g.col);
                    // A glyph under the block cursor is drawn in the background's colour.
                    let color = if block == Some((i, col)) {
                        frame.background
                    } else {
                        g.color
                    };
                    let at = point(
                        frame.origin.x + m.cell_width * col as f32 + g.dx,
                        top + m.baseline,
                    );
                    let _ = if g.glyph.emoji {
                        window.paint_emoji(at, g.glyph.font, g.glyph.id, m.font_size)
                    } else {
                        window.paint_glyph(at, g.glyph.font, g.glyph.id, m.font_size, color)
                    };
                }
                let thin = px(1.).max((m.font_size / 14.).round());
                for (span, color, stroke) in &row.strokes {
                    let b = cell(i, usize::from(span.col), usize::from(span.len));
                    let ys: &[Pixels] = match stroke {
                        Stroke::Under => &[m.baseline + thin],
                        Stroke::DoubleUnder => &[m.baseline + thin, m.baseline + thin * 3.],
                        Stroke::Strike => &[(m.line_height / 2.).round()],
                    };
                    for y in ys {
                        window.paint_quad(fill(
                            Bounds {
                                origin: point(b.origin.x, b.origin.y + *y),
                                size: size(b.size.width, thin),
                            },
                            *color,
                        ));
                    }
                }
            }
            if let Some(c) = frame.cursor {
                let b = cell(c.row, c.col, if c.wide { 2 } else { 1 });
                let thin = px(1.).max((m.font_size / 8.).round());
                match c.shape {
                    CursorShape::Beam => window.paint_quad(fill(
                        Bounds {
                            origin: b.origin,
                            size: size(thin, b.size.height),
                        },
                        c.color,
                    )),
                    CursorShape::Underline => window.paint_quad(fill(
                        Bounds {
                            origin: point(b.origin.x, b.origin.y + b.size.height - thin),
                            size: size(b.size.width, thin),
                        },
                        c.color,
                    )),
                    CursorShape::HollowBlock => {
                        window.paint_quad(gpui::outline(b, c.color, gpui::BorderStyle::Solid))
                    }
                    CursorShape::Block | CursorShape::Hidden => {}
                }
            }
        },
    );
}
