//! Replays a recorded terminal into the terminal widget in a window of its own: the real-window
//! check of the widget, and a load to measure its CPU under.
//!
//! Usage: `replay RAW TIMING [SPEED] [LOOPS] [WIDTH HEIGHT]`. `RAW` is what the program wrote and
//! `TIMING` its reads, one `seconds bytes` line each, as `tests/data` holds them. `SPEED` divides
//! the recorded delays (`max` feeds each read on the next tick), `LOOPS` replays it that many
//! times, starting the screen afresh each time. The window stays open after the last loop and
//! the widget's counters are printed after each one.

use std::time::{Duration, Instant};

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use x100_app::term::{Sizing, TerminalView};

struct Args {
    bytes: Vec<u8>,
    reads: Vec<(f64, usize)>,
    speed: Option<f64>,
    loops: usize,
    window: (f32, f32),
}

fn args() -> Result<Args, String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [raw, timing, rest @ ..] = a.as_slice() else {
        return Err("usage: replay RAW TIMING [SPEED] [LOOPS] [WIDTH HEIGHT]".into());
    };
    let bytes = std::fs::read(raw).map_err(|e| format!("{raw}: {e}"))?;
    let reads = std::fs::read_to_string(timing)
        .map_err(|e| format!("{timing}: {e}"))?
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect();
    let speed = match rest.first().map(String::as_str) {
        None => Some(1.),
        Some("max") => None,
        Some(s) => Some(s.parse().map_err(|_| format!("not a speed: {s}"))?),
    };
    let loops = rest
        .get(1)
        .map_or(Ok(1), |s| s.parse())
        .map_err(|_| "not a count")?;
    let window = match (rest.get(2), rest.get(3)) {
        (Some(w), Some(h)) => (
            w.parse().map_err(|_| "not a width")?,
            h.parse().map_err(|_| "not a height")?,
        ),
        _ => (1000., 700.),
    };
    Ok(Args {
        bytes,
        reads,
        speed,
        loops,
        window,
    })
}

fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let ran = x100_app::platform::run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(a.window.0), px(a.window.1)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("x100 replay".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| TerminalView::new(Sizing::Recorded, 120, 40, cx)),
            )
            .expect("a window");
        x100_app::platform::started();
        let view = window.entity(cx).expect("the view");
        cx.spawn(async move |cx| {
            for n in 0..a.loops {
                let start = Instant::now();
                view.update(cx, |v, cx| v.feed(b"\x1bc", cx));
                let (mut at, mut last) = (0, 0.);
                for (t, len) in &a.reads {
                    let wait = a.speed.map_or(Duration::from_millis(1), |s| {
                        Duration::from_secs_f64(((t - last) / s).max(0.))
                    });
                    last = *t;
                    cx.background_executor().timer(wait).await;
                    let chunk = &a.bytes[at..(at + len).min(a.bytes.len())];
                    at += len;
                    view.update(cx, |v, cx| v.feed(chunk, cx));
                }
                let stats = view.update(cx, |v, _| v.stats());
                eprintln!(
                    "loop {} done in {:.2}s: {stats:?}",
                    n + 1,
                    start.elapsed().as_secs_f64()
                );
            }
        })
        .detach();
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
    if let Err(e) = ran {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
