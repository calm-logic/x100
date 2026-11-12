//! A list of rows that update in place. Each row is its own entity, embedded as a cached view,
//! so notifying one row re-renders that row and nothing else. Every row counts its renders.

use std::time::Duration;

use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, StyleRefinement, Styled, Window, div,
    prelude::*, px, rgb,
};
use rand::Rng;

use crate::stats;

const ROW_HEIGHT: f32 = 22.;

pub struct RowView {
    index: usize,
    value: i64,
    updates: u64,
    renders: u64,
    /// Set by an update, cleared by the render that shows it.
    pending: bool,
}

impl Render for RowView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.renders > 0 && !self.pending {
            stats::bump(&stats::ROW_UNPROMPTED);
        }
        self.renders += 1;
        self.pending = false;
        stats::bump(&stats::ROW_RENDERS);
        let colour = if self.value >= 0 {
            rgb(0x7ee787)
        } else {
            rgb(0xff7b72)
        };
        div()
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_2()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(0x30363d))
            .whitespace_nowrap()
            .child(
                div()
                    .w(px(72.))
                    .text_color(rgb(0x8b949e))
                    .child(format!("agent-{:03}", self.index)),
            )
            .child(
                div()
                    .w(px(70.))
                    .text_color(colour)
                    .child(format!("{:+}", self.value)),
            )
            .child(
                div()
                    .text_color(rgb(0x6e7681))
                    .child(format!("upd {} · rnd {}", self.updates, self.renders)),
            )
    }
}

pub struct ListView {
    rows: Vec<Entity<RowView>>,
    cached: bool,
}

impl ListView {
    pub fn new(
        count: usize,
        interval: Duration,
        per_tick: usize,
        cached: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = (0..count)
            .map(|index| {
                cx.new(|_| RowView {
                    index,
                    value: 0,
                    updates: 0,
                    renders: 0,
                    pending: false,
                })
            })
            .collect::<Vec<_>>();

        let targets = rows.clone();
        cx.spawn(async move |_this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let picks: Vec<(usize, i64)> = {
                    let mut rng = rand::rng();
                    (0..per_tick)
                        .map(|_| {
                            (
                                rng.random_range(0..targets.len()),
                                rng.random_range(-999..=999),
                            )
                        })
                        .collect()
                };
                cx.update(|cx| {
                    for (ix, delta) in &picks {
                        targets[*ix].update(cx, |row, cx| {
                            row.value += delta;
                            row.updates += 1;
                            row.pending = true;
                            stats::bump(&stats::ROW_UPDATES);
                            cx.notify();
                        });
                    }
                });
            }
        })
        .detach();

        Self { rows, cached }
    }
}

impl Render for ListView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        stats::bump(&stats::LIST_RENDERS);
        let cached = self.cached;
        div()
            .id("list")
            .size_full()
            .overflow_y_scroll()
            .font_family("DejaVu Sans Mono")
            .text_size(px(12.))
            .children(self.rows.iter().map(|row| {
                if cached {
                    row.clone()
                        .cached(StyleRefinement::default().w_full().h(px(ROW_HEIGHT)))
                        .into_any_element()
                } else {
                    row.clone().into_any_element()
                }
            }))
    }
}
