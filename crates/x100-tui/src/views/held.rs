//! Actions the gate holds for the builder, with Approve and Deny.

use std::collections::HashMap;

use x100_client::wording::{self, Names};
use x100_client::words;

use super::Ctx;
use crate::page::{BAD, BOLD, DIM, Page, Target, WARN, span, spans};

pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    let files = HashMap::new();
    let names = Names {
        project: c.p,
        daemon: c.d,
        files: &files,
    };
    let mut held: Vec<_> = c.p.held_actions().collect();
    held.sort_by_key(|a| (a.proposed, a.action.id));
    if held.is_empty() {
        page.text("Nothing waits for you.");
        return page;
    }
    page.styled("a approves the selected action, d denies it.", DIM);
    page.blank();
    for a in held {
        let who = super::role(c.p, a.action.agent);
        let what = wording::request(&names, &a.action.request);
        let kind = a
            .classification
            .as_ref()
            .map(|c| wording::kind(c.kind).to_owned())
            .unwrap_or_default();
        let risk = a.classification.as_ref().map(|c| c.risk);
        let why = a
            .classification
            .as_ref()
            .map_or_else(String::new, |c| c.why.clone());
        let mut line = vec![
            span(who, BOLD),
            span(format!(" wants to {what}"), Default::default()),
        ];
        if let Some(r) = risk {
            line.push(span(
                format!("  [{}]", words::risk(r)),
                if matches!(r, x100_core::action::Risk::High) {
                    BAD
                } else {
                    WARN
                },
            ));
        }
        page.row(spans(line), Target::Action(a.action.id));
        if !kind.is_empty() || !why.is_empty() {
            page.paragraph(&format!("{kind}: {why}"), "    ", c.width);
        }
    }
    page
}
