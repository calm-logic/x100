//! The Log tab: every action by every agent, newest first.

use crate::logfeed::LogFeed;
use crate::page::{BOLD, DIM, Page, cell, clip, span, spans};

pub fn page(feed: &LogFeed, width: usize) -> Page {
    let mut page = Page::new();
    if feed.lines.is_empty() {
        page.text("Nothing has happened yet. Every action by every agent is listed here.");
        return page;
    }
    for r in &feed.lines {
        let used = 17 + 16;
        page.line(spans(vec![
            span(cell(&r.at, 16), DIM),
            span(" ", Default::default()),
            span(cell(&r.who, 14), BOLD),
            span(
                clip(&r.text, width.saturating_sub(used + 1)),
                Default::default(),
            ),
        ]));
    }
    page
}
