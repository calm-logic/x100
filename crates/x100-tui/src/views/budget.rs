//! The Budget tab: spend against the total, each lead's budget and each agent's spend.

use x100_client::words;
use x100_core::fold::BudgetLine;
use x100_core::money::Money;

use super::Ctx;
use crate::page::{BAD, BOLD, DIM, Page, Target, cell, span, spans};

fn share(spent: Money, of: Money) -> usize {
    if of.micros() == 0 {
        return if spent.micros() > 0 { 100 } else { 0 };
    }
    (u128::from(spent.micros()) * 100 / u128::from(of.micros())).min(100) as usize
}

fn meter(percent: usize) -> String {
    let filled = percent / 5;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(20 - filled))
}

fn of(l: &BudgetLine) -> String {
    format!("{} of {}", words::money(l.spent), words::money(l.budget))
}

pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    let Some(v) = c.p.budget() else {
        return page;
    };
    page.line(spans(vec![
        span(words::money(v.project.spent), BOLD),
        span(
            format!(
                " spent of this project's {} budget",
                words::money(v.project.budget)
            ),
            Default::default(),
        ),
    ]));
    page.text(meter(share(v.project.spent, v.project.budget)));
    if v.project.stopped {
        page.styled(
            "The project has reached its budget. No new turns start until it is raised.",
            BAD,
        );
    }
    page.styled(
        "b sets the project budget, e sets the selected lead's.",
        DIM,
    );
    let u = &v.usage;
    page.paragraph(
        &format!(
            "{} tokens so far: {} in, {} out, {} read from cache, {} written to cache.",
            words::tokens(u.all_input() + u.output),
            words::tokens(u.input),
            words::tokens(u.output),
            words::tokens(u.cache_read),
            words::tokens(u.cache_write_short + u.cache_write_long),
        ),
        "",
        c.width,
    );
    page.blank().heading("Leads");
    if v.leads.is_empty() {
        page.text("  No leads yet.");
    }
    let mut leads: Vec<_> = v.leads.iter().collect();
    leads.sort_by_key(|(id, _)| c.p.agent(**id).map(|a| (a.joined, a.role.clone())));
    for (id, l) in leads {
        let mut parts = vec![
            span(
                format!("  {}", cell(&super::role(c.p, *id), 14)),
                Default::default(),
            ),
            span(format!("{} ", meter(share(l.spent, l.budget))), DIM),
            span(of(l), Default::default()),
        ];
        if l.stopped {
            parts.push(span("  Stopped at its budget", BAD));
        }
        page.row(spans(parts), Target::Lead(*id));
    }
    page.blank().heading("Lab");
    let mut lab = vec![
        span(
            format!("  {}", meter(share(v.lab.spent, v.lab.budget))),
            DIM,
        ),
        span(format!(" {}", of(&v.lab)), Default::default()),
    ];
    if v.lab.stopped {
        lab.push(span("  Stopped at its budget", BAD));
    }
    page.line(spans(lab));
    page.blank().heading("Spend per agent");
    let mut agents: Vec<_> = c.p.agents().collect();
    let ceo = c.p.project().map(|p| p.ceo);
    agents.sort_by_key(|a| (Some(a.id) != ceo, a.joined, a.role.clone()));
    for a in agents {
        let spent = v.agents.get(&a.id).map(|s| s.cost).unwrap_or_default();
        page.text(format!("  {}{}", cell(&a.role, 14), words::money(spent)));
    }
    page
}
