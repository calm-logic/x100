//! Liquid glass, as on the marketing page's mock: the primary action button and the piece of glass
//! a toggle's chosen option sits on.
//!
//! GPUI paints quads, linear gradients, drop and inset shadows and paths, clipped to rectangles. It
//! has no radial gradient, no blur filter and no transform of laid-out elements, so the glass is
//! painted on a canvas behind its label: the radial highlight is a linear gradient with a soft
//! inset glow, the blurred light drifting inside is inset glows pushed off centre, the ripple is a
//! ring kept to the straight middle of the pill, and a press squashes the glass while the label
//! holds still.
//!
//! Motion costs nothing when nothing happens: hover, press and slide run frame by frame only while
//! they last, and the light inside a button drifts only while the pointer is on it, a few frames a
//! second, in the active window, and not under reduced motion. On the mock it drifts always; here
//! that redrew the window a dozen times a second for as long as a button was on screen, which
//! measured at 40% of a core in a debug build.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, BoxShadow, ClickEvent, ContentMask, Corners, ElementId, Hsla,
    MouseButton, Pixels, Point, SharedString, Window, canvas, div, hsla, linear_color_stop,
    linear_gradient, point, prelude::*, px, quad, size,
};

use crate::theme;

fn white(a: f32) -> Hsla {
    hsla(0., 0., 1., a)
}

/// How long a hover takes to brighten the glass, and a press to squash and settle.
const HOVER: f32 = 0.5;
const PRESS: f32 = 0.6;
const RIPPLE: f32 = 0.9;
/// One turn of the light inside, as `pab-flow` on the mock.
const FLOW: f32 = 11.;
/// The light inside moves slowly, so a few frames a second keep it smooth.
const FLOW_FPS: f32 = 12.;

#[derive(Clone, Copy, PartialEq)]
pub struct Look {
    /// 0 at rest to 1 hovered.
    pub hover: f32,
    /// The glow around the outside, as its blur. A button inside a card that glows has none.
    pub outer_glow: Option<f32>,
}

/// Paints a pill of glass filling `rect`.
pub fn paint_glass(window: &mut Window, rect: Bounds<Pixels>, look: Look) {
    let h = look.hover;
    let r = Corners::all(rect.size.height.min(rect.size.width) / 2.);
    if let Some(blur) = look.outer_glow {
        window.paint_drop_shadows(
            rect,
            r,
            &[shadow(white(0.15 + 0.14 * h), 0., blur + 16. * h, false)],
        );
    }
    // The mock's highlight is radial from above the top left; a diagonal fade over the dark body,
    // lightest where that light falls, is the nearest GPUI paints.
    let top = hsla(0., 0., 0.17 + 0.05 * h, 1.);
    let bottom = hsla(0., 0., 0.06, 1.);
    let fill = linear_gradient(
        160.,
        linear_color_stop(top, 0.),
        linear_color_stop(bottom, 0.6),
    );
    window.paint_quad(quad(
        rect,
        r,
        fill,
        px(0.),
        theme::transparent(),
        Default::default(),
    ));
    window.paint_inset_shadows(
        rect,
        r,
        &[
            // Light pooled at the bottom of the glass.
            shadow_at(white(0.1 + 0.06 * h), point(px(0.), px(-12.)), 20. + 2. * h),
            // The soft glow all round inside.
            shadow(white(0.14 + 0.08 * h), 0., 24. + 6. * h, true),
            // The bright top edge.
            shadow_at(white(0.95 + 0.05 * h), point(px(0.), px(1.5)), 0.),
        ],
    );
    window.paint_quad(quad(
        rect,
        r,
        theme::transparent(),
        px(1.),
        white(0.6 + 0.25 * h),
        Default::default(),
    ));
}

fn shadow(color: Hsla, offset: f32, blur: f32, inset: bool) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(offset), px(offset)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset,
    }
}

fn shadow_at(color: Hsla, offset: Point<Pixels>, blur: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset,
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: true,
    }
}

/// A CSS `cubic-bezier` easing at `t`.
pub fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1. - s;
        3. * u * u * s * a + 3. * u * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0f32, 1f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(y1, y2, (lo + hi) / 2.)
}

/// The mock's `pab-press`: wide and short, then narrow and tall, then settled.
fn squash(t: f32) -> (f32, f32) {
    const KEYS: [(f32, f32, f32); 5] = [
        (0., 1., 1.),
        (0.22, 1.05, 0.86),
        (0.48, 0.96, 1.07),
        (0.72, 1.02, 0.98),
        (1., 1., 1.),
    ];
    let t = t.clamp(0., 1.);
    for w in KEYS.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            // As in CSS, the easing applies between each pair of keyframes.
            let k = bezier(0.2, 1.4, 0.4, 1., (t - a.0) / (b.0 - a.0));
            return (a.1 + (b.1 - a.1) * k, a.2 + (b.2 - a.2) * k);
        }
    }
    (1., 1.)
}

/// Asks for another frame now, or for a redraw of the current view after `after`.
fn again(window: &mut Window, cx: &mut App, after: Option<Duration>, pending: &Rc<RefCell<bool>>) {
    match after {
        None => window.request_animation_frame(),
        Some(after) if !*pending.borrow() => {
            *pending.borrow_mut() = true;
            let pending = Rc::clone(pending);
            let view = window.current_view();
            window
                .spawn(cx, async move |cx| {
                    cx.background_executor().timer(after).await;
                    *pending.borrow_mut() = false;
                    cx.update(move |_, cx| cx.notify(view)).ok();
                })
                .detach();
        }
        Some(_) => {}
    }
}

/// Whether slow, decorative motion may run now.
fn may_drift(window: &Window, cx: &App) -> bool {
    !cx.reduce_motion() && window.is_window_active()
}

#[derive(Default)]
struct Pab {
    hovered: bool,
    /// When the hover last changed, and the glass's brightness then.
    hover_changed: Option<(Instant, f32)>,
    pointer: Option<Point<Pixels>>,
    press: Option<(Instant, Point<Pixels>)>,
    /// How far the light inside has turned while hovered before, and since when it is turning.
    turned: f32,
    turning_since: Option<Instant>,
    drift_pending: Rc<RefCell<bool>>,
}

impl Pab {
    fn hover(&self, now: Instant) -> f32 {
        let target = if self.hovered { 1. } else { 0. };
        match self.hover_changed {
            None => target,
            Some((at, from)) => {
                let t = (now - at).as_secs_f32() / HOVER;
                from + (target - from) * bezier(0.2, 1., 0.4, 1., t)
            }
        }
    }
}

type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The primary action button: a pill of liquid glass, the action's label on it.
#[derive(IntoElement)]
pub struct Primary {
    id: ElementId,
    label: SharedString,
    outer_glow: bool,
    selector: Option<String>,
    on_click: Option<OnClick>,
}

/// A primary button. Every primary action in the app is one of these.
pub fn primary(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Primary {
    Primary {
        id: id.into(),
        label: label.into(),
        outer_glow: true,
        selector: None,
        on_click: None,
    }
}

impl Primary {
    /// Inside a card that glows itself, such as the message box: the glass rim only.
    pub fn rim_only(mut self) -> Self {
        self.outer_glow = false;
        self
    }

    pub fn selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Primary {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| Pab::default());
        let outer_glow = self.outer_glow;
        let glass = {
            let state = state.clone();
            canvas(
                |_, _, _| (),
                move |bounds, (), window, cx| {
                    let now = Instant::now();
                    let (hover, press, pointer, turned, pending, hovering) = {
                        let s = state.read(cx);
                        (
                            s.hover(now),
                            s.press,
                            s.pointer,
                            s.turned + s.turning_since.map_or(0., |at| (now - at).as_secs_f32()),
                            Rc::clone(&s.drift_pending),
                            s.hovered,
                        )
                    };
                    let since = |at: Instant| (now - at).as_secs_f32();
                    let pressed = press.filter(|(at, _)| since(*at) < RIPPLE);
                    let (sx, sy) = match pressed {
                        Some((at, _)) if !cx.reduce_motion() => squash(since(at) / PRESS),
                        _ => (1., 1.),
                    };
                    // Hovered, it lifts a pixel and grows a little, as `.pab:hover` does.
                    let grow = 1. + 0.02 * hover;
                    let (w, h) = (
                        bounds.size.width * sx * grow,
                        bounds.size.height * sy * grow,
                    );
                    let centre = bounds.center() - point(px(0.), px(hover));
                    let rect = Bounds::new(centre - point(w / 2., h / 2.), size(w, h));
                    let outer_glow = outer_glow.then_some(28.);
                    paint_glass(window, rect, Look { hover, outer_glow });
                    let drift = hovering && may_drift(window, cx);
                    // Three lights turning slowly inside the glass, as `pab::before`. Each is an
                    // inset glow pushed off centre, which GPUI clips to the pill's own curve, where
                    // a soft blob would show the square of a rectangular clip at the ends.
                    let turn = 0.6 + turned / FLOW * std::f32::consts::TAU;
                    let (fw, fh) = (f32::from(w), f32::from(h));
                    let mut lights: Vec<BoxShadow> = [(0., 0.12), (2.3, 0.08), (4.1, 0.06)]
                        .iter()
                        .map(|(phase, alpha)| {
                            let a = turn + phase;
                            let off = point(px(a.cos() * fw * 0.12), px(a.sin() * fh * 0.3));
                            shadow_at(white(*alpha), off, fh * 0.7)
                        })
                        .collect();
                    // A glow on the side the pointer is, while hovered.
                    if let Some(p) = pointer.filter(|_| hover > 0.01) {
                        let toward = rect.center() - p;
                        let off = point(toward.x * 0.35, toward.y * 0.5);
                        lights.push(shadow_at(white(0.2 * hover), off, fh * 0.8));
                    }
                    window.paint_inset_shadows(rect, Corners::all(h / 2.), &lights);
                    // The ripple from the press, kept off the round ends, where a rectangular
                    // clip would show past the curve.
                    if let Some((at, from)) = pressed.filter(|_| !cx.reduce_motion()) {
                        let t = since(at) / RIPPLE;
                        let grown = bezier(0.16, 0.7, 0.3, 1., t);
                        let radius = px(6. + 120. * grown);
                        let ring = Bounds::new(
                            from - point(radius, radius),
                            size(radius * 2., radius * 2.),
                        );
                        let middle = Bounds::from_corners(
                            point(rect.left() + h / 2., rect.top()),
                            point(rect.right() - h / 2., rect.bottom()),
                        );
                        window.with_content_mask(Some(ContentMask { bounds: middle }), |window| {
                            window.paint_quad(quad(
                                ring,
                                Corners::all(radius),
                                theme::transparent(),
                                px(2.5),
                                white(0.6 * (1. - t)),
                                Default::default(),
                            ));
                        });
                    }
                    let moving = pressed.is_some()
                        || state
                            .read(cx)
                            .hover_changed
                            .is_some_and(|(at, _)| since(at) < HOVER);
                    if moving {
                        again(window, cx, None, &pending);
                    } else if drift {
                        again(
                            window,
                            cx,
                            Some(Duration::from_secs_f32(1. / FLOW_FPS)),
                            &pending,
                        );
                    }
                },
            )
            .absolute()
            .inset_0()
        };
        let (hover_state, move_state, press_state) = (state.clone(), state.clone(), state);
        let on_click = self.on_click;
        let selector = self.selector;
        // The label carries a second selector, so a test can tell this widget drew the action.
        let label = div()
            .relative()
            .whitespace_nowrap()
            .when_some(selector.clone(), |d, s| {
                d.debug_selector(move || format!("primary-{s}"))
            })
            .child(self.label);
        div()
            .id(self.id)
            .when_some(selector, |d, s| d.debug_selector(move || s.clone()))
            .relative()
            .flex_none()
            .px(px(15.))
            .py(px(6.))
            .rounded_full()
            .cursor_pointer()
            .text_size(px(theme::SMALL))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme::text())
            .child(glass)
            .child(label)
            .on_hover(move |hovered, window, cx| {
                let now = Instant::now();
                hover_state.update(cx, |s, cx| {
                    let from = s.hover(now);
                    if let Some(at) = s.turning_since.take() {
                        s.turned += (now - at).as_secs_f32();
                    }
                    if *hovered {
                        s.turning_since = Some(now);
                    }
                    s.hovered = *hovered;
                    s.hover_changed = Some((now, from));
                    cx.notify();
                });
                window.refresh();
            })
            .on_mouse_move(move |e, _, cx| {
                move_state.update(cx, |s, cx| {
                    s.pointer = Some(e.position);
                    cx.notify();
                });
            })
            .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                press_state.update(cx, |s, _| s.press = Some((Instant::now(), e.position)));
                window.refresh();
            })
            .when_some(on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
    }
}

#[derive(Default)]
struct Glide {
    on: Option<usize>,
    from: Option<Bounds<Pixels>>,
    to: Option<Bounds<Pixels>>,
    since: Option<Instant>,
}

/// The mock's `.seg__glass`: the edge moving toward the new choice leads and the one behind it
/// follows, both overshooting a little, so the glass stretches and settles like a drop.
fn glide(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let (lead, trail) = (0.32, 0.56);
    let ease = |d: f32| bezier(0.3, 1.25, 0.45, 1., t / d);
    let edge =
        |a: Pixels, b: Pixels, leads: bool| a + (b - a) * ease(if leads { lead } else { trail });
    let right = to.left() > from.left();
    let down = to.top() > from.top();
    let left = edge(from.left(), to.left(), !right);
    let rgt = edge(from.right(), to.right(), right);
    let top = edge(from.top(), to.top(), !down);
    let bottom = edge(from.bottom(), to.bottom(), down);
    Bounds::from_corners(point(left, top), point(rgt, bottom))
}

/// A choice of one among a few, in a pill. The chosen option sits on a piece of glass, which
/// slides to the next choice when it changes. Options are made with [`crate::widgets::segment`].
#[derive(IntoElement)]
pub struct Segments {
    id: ElementId,
    on: Option<usize>,
    items: Vec<AnyElement>,
}

pub fn segments(
    id: impl Into<ElementId>,
    on: Option<usize>,
    items: impl IntoIterator<Item = AnyElement>,
) -> Segments {
    Segments {
        id: id.into(),
        on,
        items: items.into_iter().collect(),
    }
}

impl RenderOnce for Segments {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| Glide::default());
        let spots: Rc<RefCell<Vec<Option<Bounds<Pixels>>>>> =
            Rc::new(RefCell::new(vec![None; self.items.len()]));
        let on = self.on;
        let read = Rc::clone(&spots);
        let glass = canvas(
            |_, _, _| (),
            move |_, (), window, cx| {
                let Some(target) = on.and_then(|i| read.borrow().get(i).copied().flatten()) else {
                    return;
                };
                let now = Instant::now();
                let still = cx.reduce_motion();
                let rect = state.update(cx, |g, _| {
                    let moved = g.on.is_some() && g.on != on;
                    let shown = match (g.from, g.to, g.since) {
                        (Some(f), Some(t), Some(at)) => glide(f, t, (now - at).as_secs_f32()),
                        _ => target,
                    };
                    if moved && !still {
                        g.from = Some(shown);
                        g.since = Some(now);
                    } else if g.to != Some(target) && !moved {
                        // Laid out again, as when the window is resized: no slide.
                        g.from = Some(target);
                        g.since = None;
                    }
                    g.on = on;
                    g.to = Some(target);
                    match (g.from, g.since) {
                        (Some(f), Some(at)) => glide(f, target, (now - at).as_secs_f32()),
                        _ => target,
                    }
                });
                let sliding = state
                    .read(cx)
                    .since
                    .is_some_and(|at| (now - at).as_secs_f32() < 0.6);
                // A toggle's glass glows less than a button's, as `.seg .pab` on the mock.
                let look = Look {
                    hover: 0.,
                    outer_glow: Some(14.),
                };
                paint_glass(window, rect, look);
                if sliding {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .inset_0();
        let items = self.items.into_iter().enumerate().map(|(i, item)| {
            let write = Rc::clone(&spots);
            div()
                .relative()
                .child(
                    canvas(
                        move |b, _, _| {
                            if let Some(slot) = write.borrow_mut().get_mut(i) {
                                *slot = Some(b);
                            }
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(item)
        });
        div()
            .id(self.id)
            .relative()
            .flex()
            .p(px(3.))
            .gap(px(2.))
            .rounded_full()
            .border_1()
            .border_color(theme::line())
            .child(glass)
            .children(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_easings_start_and_end_where_they_should() {
        for (x1, y1, x2, y2) in [(0.3, 1.25, 0.45, 1.), (0.2, 1.4, 0.4, 1.)] {
            assert!(bezier(x1, y1, x2, y2, 0.).abs() < 1e-3);
            assert!((bezier(x1, y1, x2, y2, 1.) - 1.).abs() < 1e-3);
        }
        assert_eq!(squash(1.), (1., 1.));
        let (sx, sy) = squash(0.12);
        assert!(
            sx > 1. && sy < 1.,
            "wide and short early in a press: {sx} {sy}"
        );
    }
}
