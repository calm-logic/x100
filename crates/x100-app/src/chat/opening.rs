//! The first run's opening: a glass orb at the centre spreads at its edges, squares off and grows
//! into the box for the first message, while a rim grows from it out to the window's edges and the
//! window comes up from black behind it. It lasts under a second, any click or key ends it, and it
//! is not played under reduced motion or once a project exists.
//!
//! It is painted on a canvas laid over the box, in window coordinates, so its last frame is the
//! box itself and the real box is there when the glass fades. GPUI has no shader or blur for a
//! liquid surface, so the glass is a rounded shape with a soft rim, an inner glow and a highlight,
//! and the "liquid" is its edges wobbling out of round as it spreads.

use std::time::{Duration, Instant};

use gpui::{
    Bounds, Corners, Hsla, Pixels, Window, canvas, hsla, linear_color_stop, linear_gradient, point,
    prelude::*, px, quad, size,
};

use crate::glass::bezier;
use crate::theme;

pub const LENGTH: Duration = Duration::from_millis(750);

/// The box's own corners and fill, which the glass ends on.
const BOX_RADIUS: f32 = 12.;

fn white(a: f32) -> Hsla {
    hsla(0., 0., 1., a)
}

/// Where in `from..to` of the whole the time `t` is, eased out, 0 before and 1 after.
fn phase(t: f32, from: f32, to: f32) -> f32 {
    bezier(0.22, 1., 0.36, 1., ((t - from) / (to - from)).clamp(0., 1.))
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

fn mix(a: Bounds<Pixels>, b: Bounds<Pixels>, k: f32) -> Bounds<Pixels> {
    let l = |x: Pixels, y: Pixels| px(lerp(f32::from(x), f32::from(y), k));
    Bounds::from_corners(
        point(l(a.left(), b.left()), l(a.top(), b.top())),
        point(l(a.right(), b.right()), l(a.bottom(), b.bottom())),
    )
}

/// The canvas, to lay over the box with `absolute().inset_0()`. `started` is when the first frame
/// was drawn.
pub fn overlay(started: Instant) -> impl IntoElement {
    // Painted after everything else, so the window behind it, the sidebar included, rises from it.
    let frames = canvas(
        |_, _, _| (),
        move |boxed, (), window, _| {
            let t = started.elapsed().as_secs_f32() / LENGTH.as_secs_f32();
            paint(window, boxed, t);
            if t < 1. {
                window.request_animation_frame();
            }
        },
    )
    .size_full();
    gpui::div()
        .absolute()
        .inset_0()
        .child(gpui::deferred(frames))
}

/// One frame at `t` from 0 to 1, the box at `boxed`.
pub fn paint(window: &mut Window, boxed: Bounds<Pixels>, t: f32) {
    let view = window.viewport_size();
    let whole = Bounds::new(point(px(0.), px(0.)), view);

    // The window rises from black behind the glass.
    let veil = 1. - phase(t, 0.45, 0.95);
    if veil > 0. {
        let bg = Hsla::from(theme::bg());
        window.paint_quad(gpui::fill(whole, hsla(bg.h, bg.s, bg.l, veil)));
    }

    // A rim leaves the box and grows to the window's edges, fading as it reaches them.
    let reach = phase(t, 0.3, 0.85);
    if reach > 0. && reach < 1. {
        let rim = mix(boxed, whole.dilate(px(-1.)), reach);
        window.paint_quad(quad(
            rim,
            Corners::all(px(lerp(BOX_RADIUS, 10., reach))),
            theme::transparent(),
            px(1.5),
            white(0.55 * (1. - reach)),
            Default::default(),
        ));
    }

    // The orb: it swells in, its edges wobble out of round as it spreads, and it squares off into
    // the box.
    let born = phase(t, 0., 0.3);
    let spread = phase(t, 0.18, 0.7);
    let d = 88. * lerp(0.55, 1., bezier(0.34, 1.56, 0.64, 1., born.min(1.)));
    let wobble = (t * 22.).sin() * 0.09 * (1. - spread) * born;
    let orb = Bounds::new(
        boxed.center() - point(px(d * (1. + wobble) / 2.), px(d * (1. - wobble) / 2.)),
        size(px(d * (1. + wobble)), px(d * (1. - wobble))),
    );
    let shape = mix(orb, boxed, spread);
    let short = f32::from(shape.size.height.min(shape.size.width));
    let radius = px(lerp(short / 2., BOX_RADIUS, spread).min(short / 2.));
    let solid = 1. - phase(t, 0.62, 1.);
    if solid <= 0. {
        return;
    }
    let corners = Corners::all(radius);
    window.paint_drop_shadows(
        shape,
        corners,
        &[gpui::BoxShadow {
            color: white(0.28 * solid * (1. - spread * 0.6)),
            offset: point(px(0.), px(0.)),
            blur_radius: px(lerp(30., 22., spread)),
            spread_radius: px(0.),
            inset: false,
        }],
    );
    // Glass at first, the box's own fill by the end.
    let glassy = 1. - spread;
    let top = hsla(0., 0., lerp(0.095, 0.24, glassy), solid);
    let bottom = hsla(0., 0., lerp(0.095, 0.05, glassy), solid);
    let fill = linear_gradient(
        160.,
        linear_color_stop(top, 0.),
        linear_color_stop(bottom, 0.65),
    );
    window.paint_quad(quad(
        shape,
        corners,
        fill,
        px(0.),
        theme::transparent(),
        Default::default(),
    ));
    window.paint_inset_shadows(
        shape,
        corners,
        &[
            gpui::BoxShadow {
                color: white(0.18 * solid * glassy.max(0.3)),
                offset: point(px(0.), px(0.)),
                blur_radius: px(lerp(26., 14., spread)),
                spread_radius: px(0.),
                inset: true,
            },
            gpui::BoxShadow {
                color: white(0.9 * solid * glassy),
                offset: point(px(0.), px(1.5)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: true,
            },
        ],
    );
    // A highlight in the top left, the light the glass catches, gone as it becomes the box.
    if glassy > 0.3 {
        let h = shape.size.height;
        let hl = Bounds::new(
            shape.origin + point(h * 0.22, h * 0.14),
            size(h * 0.34, h * 0.2),
        );
        let r = hl.size.height.min(hl.size.width) / 2.;
        window.paint_drop_shadows(
            hl,
            Corners::all(r),
            &[gpui::BoxShadow {
                color: white(0.3 * glassy * glassy * solid),
                offset: point(px(0.), px(0.)),
                blur_radius: h * 0.08,
                spread_radius: px(0.),
                inset: false,
            }],
        );
    }
    window.paint_quad(quad(
        shape,
        corners,
        theme::transparent(),
        px(1.),
        white(lerp(0.75, 0.3, spread) * solid),
        Default::default(),
    ));
}
