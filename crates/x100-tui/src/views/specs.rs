//! The Specs tab: BRD, PRD and ADD, each with its author, revision and status.

use x100_client::words;
use x100_core::spec::{SpecKind, SpecStatus};

use super::Ctx;
use crate::page::{ACCENT, BOLD, DIM, Page, span, spans};

pub const KINDS: [SpecKind; 3] = [SpecKind::Brd, SpecKind::Prd, SpecKind::Add];

pub fn page(c: &Ctx, open: SpecKind) -> Page {
    let mut page = Page::new();
    page.line(spans(
        KINDS
            .iter()
            .flat_map(|k| {
                let style = if *k == open { ACCENT.patch(BOLD) } else { DIM };
                [
                    span(format!(" {} ", words::spec_abbreviation(*k)), style),
                    span(" ", Default::default()),
                ]
            })
            .collect(),
    ));
    page.styled("Left and right switch document.", DIM);
    page.blank();
    match c.p.spec(open) {
        None => {
            page.text(format!(
                "No {} has been written yet.",
                words::spec_name(open)
            ));
        }
        Some(s) => {
            let s = &s.spec;
            page.styled(
                format!(
                    "{} · by {} · revision {} · {}",
                    words::spec_name(open),
                    super::role(c.p, s.author),
                    s.revision,
                    match s.status {
                        SpecStatus::Draft => "Draft",
                        SpecStatus::Approved => "Approved",
                    }
                ),
                BOLD,
            );
            page.blank();
            for line in s.body.lines() {
                page.paragraph(line, "", c.width);
            }
        }
    }
    page
}
