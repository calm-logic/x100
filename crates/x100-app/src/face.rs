//! An agent's face: its outline drawn as a line in its colour, open inside, with two eyes. Every
//! face in the app, the CEO's included, is drawn here, so a change of look shows the same
//! everywhere.

use gpui::{Bounds, Div, PathBuilder, Pixels, Point, Window, canvas, div, point, prelude::*, px};
use x100_core::appearance::{Appearance, Outline};

use crate::{theme, words};

/// A face `size` pixels across in `look`. Tests find any face by the selector `face`, and its look
/// by `face-<outline>-<colour>`.
pub fn face(size: f32, look: Appearance) -> Div {
    div().flex_none().size(px(size)).child(
        div().debug_selector(|| "face".into()).size_full().child(
            div()
                .debug_selector(move || selector(look))
                .size_full()
                .child(canvas(|_, _, _| (), move |b, (), w, _| paint(b, look, w)).size_full()),
        ),
    )
}

/// The room a face takes, for a row whose actor is not an agent.
pub fn blank(size: f32) -> Div {
    div().flex_none().size(px(size))
}

pub fn selector(look: Appearance) -> String {
    let hue = words::hue(look.hue).to_lowercase().replace(' ', "-");
    format!("face-{}-{hue}", words::outline(look.outline).to_lowercase())
}

fn paint(bounds: Bounds<Pixels>, look: Appearance, window: &mut Window) {
    let colour = theme::hue(look.hue);
    let s = f32::from(bounds.size.width.min(bounds.size.height));
    // Thin enough for a row, heavy enough to read at the Team tab's size.
    let line = (s / 24.).clamp(1.25, 2.5);
    let o = bounds.origin;
    let at = |x: f32, y: f32| point(o.x + px(x * s), o.y + px(y * s));
    // The line is centred on the outline, so the outline sits half a line inside the bounds.
    let m = (line / 2. + 0.5) / s;
    let shape = outline(look.outline, m);
    let mut path = PathBuilder::stroke(px(line));
    let points: Vec<Point<Pixels>> = shape.iter().map(|(x, y)| at(*x, *y)).collect();
    path.add_polygon(&points, true);
    if let Ok(path) = path.build() {
        window.paint_path(path, colour);
    }
    let eyes = eyes(look.outline);
    for x in [0.5 - eyes.apart / 2., 0.5 + eyes.apart / 2.] {
        let (w, h) = (eyes.w * s, eyes.h * s);
        let b = Bounds::new(
            at(x - eyes.w / 2., eyes.y - eyes.h / 2.),
            gpui::size(px(w), px(h)),
        );
        window.paint_quad(gpui::fill(b, colour).corner_radii(px(w / 2.)));
    }
}

struct Eyes {
    /// The eyes' centre, down from the top, as a share of the size.
    y: f32,
    /// From one eye's centre to the other's.
    apart: f32,
    w: f32,
    h: f32,
}

/// Where the eyes sit: in the widest part of the outline, smaller where it is narrow.
fn eyes(outline: Outline) -> Eyes {
    let (y, apart, w, h) = match outline {
        Outline::Triangle => (0.64, 0.22, 0.12, 0.17),
        Outline::Diamond => (0.48, 0.22, 0.12, 0.18),
        Outline::Shield => (0.42, 0.3, 0.14, 0.2),
        Outline::Capsule => (0.48, 0.3, 0.13, 0.19),
        _ => (0.46, 0.32, 0.16, 0.22),
    };
    Eyes { y, apart, w, h }
}

/// The outline as a closed line in a unit square, `m` in from its edges.
fn outline(outline: Outline, m: f32) -> Vec<(f32, f32)> {
    let (lo, hi) = (m, 1. - m);
    let r = 0.5 - m;
    let around = |n: usize, start: f32, radius: f32| -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let a = start + i as f32 * std::f32::consts::TAU / n as f32;
                (0.5 + radius * a.cos(), 0.5 + radius * a.sin())
            })
            .collect()
    };
    let deg = |d: f32| d.to_radians();
    match outline {
        Outline::Circle => around(72, 0., r),
        Outline::Square => soft(&[(lo, lo), (hi, lo), (hi, hi), (lo, hi)], 0.24),
        Outline::Hexagon => soft(&around(6, deg(-90.), r), 0.1),
        Outline::Octagon => soft(&around(8, deg(22.5), r / deg(22.5).cos()), 0.06),
        Outline::Diamond => soft(&around(4, deg(-90.), r), 0.12),
        Outline::Triangle => soft(&[(0.5, lo), (hi, hi - 0.03), (lo, hi - 0.03)], 0.14),
        Outline::Shield => soft(
            &[
                (lo + 0.04, lo + 0.04),
                (hi - 0.04, lo + 0.04),
                (hi - 0.04, 0.5),
                (0.5, hi),
                (lo + 0.04, 0.5),
            ],
            0.14,
        ),
        Outline::Capsule => capsule(lo, hi, 0.2),
    }
}

/// A polygon with each corner cut by a curve that starts `k` along each side from it, at most half
/// the side.
fn soft(corners: &[(f32, f32)], k: f32) -> Vec<(f32, f32)> {
    let n = corners.len();
    let mut out = Vec::with_capacity(n * 9);
    for i in 0..n {
        let prev = corners[(i + n - 1) % n];
        let p = corners[i];
        let next = corners[(i + 1) % n];
        let toward = |q: (f32, f32)| {
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let len = (dx * dx + dy * dy).sqrt();
            let t = k.min(len / 2.) / len;
            (p.0 + dx * t, p.1 + dy * t)
        };
        let (a, b) = (toward(prev), toward(next));
        for j in 0..=8 {
            let t = j as f32 / 8.;
            let u = 1. - t;
            out.push((
                u * u * a.0 + 2. * u * t * p.0 + t * t * b.0,
                u * u * a.1 + 2. * u * t * p.1 + t * t * b.1,
            ));
        }
    }
    out
}

/// A stadium lying down: wider than tall, round at both ends.
fn capsule(lo: f32, hi: f32, inset: f32) -> Vec<(f32, f32)> {
    let (top, bottom) = (lo + inset, hi - inset);
    let r = (bottom - top) / 2.;
    let cy = (top + bottom) / 2.;
    let mut out = Vec::new();
    let half = |cx: f32, from: f32| {
        (0..=24).map(move |i| {
            let a = from + i as f32 * std::f32::consts::PI / 24.;
            (cx + r * a.cos(), cy + r * a.sin())
        })
    };
    out.extend(half(hi - r, -std::f32::consts::FRAC_PI_2));
    out.extend(half(lo + r, std::f32::consts::FRAC_PI_2));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every outline stays inside its square, so the line is never cut by the face's bounds.
    #[test]
    fn every_outline_fits_its_square() {
        for o in Outline::ALL {
            for (x, y) in outline(o, 0.05) {
                assert!(
                    (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                    "{o:?}"
                );
            }
        }
    }
}
