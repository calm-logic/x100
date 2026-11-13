//! The Team tab: the tree of agents with what each is on, and the list of drivers.

use ratatui::text::Line;
use x100_client::{output_log, words};
use x100_core::agent::AgentState;
use x100_core::id::AgentId;

use super::Ctx;
use crate::page::{ACCENT, DIM, GOOD, Page, Target, WARN, cell, clip, span, spans};

/// "● Tech  Working  Build the site  50%  Claude Code · sonnet · Medium  Medium to Max".
pub fn agent_line(c: &Ctx, id: AgentId, depth: usize) -> Option<Line<'static>> {
    let row = c.p.row(id)?;
    let state = row.agent.state.clone();
    let dot = match state {
        AgentState::Working => span("● ", GOOD),
        AgentState::Retired => span("○ ", DIM),
        AgentState::Waiting(_) => span("◐ ", WARN),
        _ => span("○ ", ACCENT),
    };
    let task = row.task.map_or_else(String::new, |t| t.task.title.clone());
    let progress = row
        .progress
        .map_or_else(String::new, |p| format!("{}%", p.value()));
    let driver = row
        .driver
        .map_or_else(String::new, |d| output_log::driver_label(c.d, &d.driver));
    let range = format!(
        "{} to {}",
        words::effort(row.effort.min()),
        words::effort(row.effort.max())
    );
    let indent = depth * 2;
    let mut line = vec![
        span(" ".repeat(indent), DIM),
        dot,
        span(
            cell(&row.agent.role, 20usize.saturating_sub(indent)),
            crate::page::BOLD,
        ),
        span(cell(words::agent_state(&state), 16), DIM),
        span(cell(&task, 24), Default::default()),
        span(cell(&progress, 5), DIM),
    ];
    // A narrow screen keeps the first columns, which say who and what.
    if c.width >= 120 {
        line.push(span(cell(&driver, 38), DIM));
        line.push(span(range, DIM));
    }
    Some(spans(line))
}

/// The agents under `id`, in the order they joined, then by role, so the order is the same on every
/// run.
fn ordered(c: &Ctx, id: AgentId) -> Vec<AgentId> {
    let mut children = c.p.children(id);
    children.sort_by_key(|a| c.p.agent(*a).map(|r| (r.joined, r.role.clone())));
    children
}

fn tree(c: &Ctx, page: &mut Page, id: AgentId, depth: usize) {
    if let Some(line) = agent_line(c, id, depth) {
        page.row(line, Target::Agent(id));
    }
    for child in ordered(c, id) {
        tree(c, page, child, depth + 1);
    }
}

/// The CEO and everyone under it.
pub fn page(c: &Ctx) -> Page {
    let mut page = Page::new();
    let Some(project) = c.p.project() else {
        return page;
    };
    page.styled("Enter opens an agent.  d lists drivers.", DIM);
    page.blank();
    tree(c, &mut page, project.ceo, 0);
    page
}

/// Every live and warm driver of the project.
pub fn drivers(c: &Ctx) -> Page {
    let mut page = Page::new();
    page.styled(
        "s stops the selected driver and hands its work on.  d lists agents.",
        DIM,
    );
    page.styled(
        format!(
            "{}{}{}{}{}{}{}{}",
            cell("AGENT", 14),
            cell("HARNESS", 22),
            cell("MODEL", 14),
            cell("EFFORT", 8),
            cell("MACHINE", 12),
            cell("STATE", 12),
            cell("CONTEXT", 9),
            "COST"
        ),
        DIM,
    );
    let mut live: Vec<_> = c.p.live_drivers().collect();
    live.sort_by_key(|d| (d.started, d.driver.id));
    for d in live {
        let agent = super::role(c.p, d.driver.agent);
        let line = format!(
            "{}{}{}{}{}{}{}{}",
            cell(&agent, 14),
            cell(&words::harness(c.d, d.driver.harness), 22),
            cell(&d.driver.model.0, 14),
            cell(words::effort(d.driver.effort), 8),
            cell(&words::machine(c.d, d.driver.machine), 12),
            cell(words::driver_state(&d.state), 12),
            cell(&words::context(d.context), 9),
            words::money(d.spent.cost)
        );
        page.row(Line::raw(line), Target::Driver(d.driver.id));
    }
    if page.targets().is_empty() {
        page.text("No driver is live.");
    }
    page
}

/// The agent's own page when it runs agents: the trail, and the agents under it.
pub fn children(c: &Ctx, agent: AgentId) -> Page {
    let mut page = Page::new();
    for child in ordered(c, agent) {
        tree(c, &mut page, child, 0);
    }
    if page.items.is_empty() {
        page.text("It runs no agents.");
    }
    page
}

/// The header of an agent's page: the trail back to the CEO, and the agent's line.
pub fn header(c: &Ctx, agent: AgentId) -> Vec<Line<'static>> {
    let trail =
        c.p.trail(agent)
            .into_iter()
            .map(|a| super::role(c.p, a))
            .collect::<Vec<_>>()
            .join(" > ");
    let mut lines = vec![Line::styled(clip(&trail, c.width), crate::page::BOLD)];
    if let Some(l) = agent_line(c, agent, 0) {
        lines.push(l);
    }
    lines
}

/// Waits, inbox and jobs of one agent, from the state.
pub fn waits(c: &Ctx, agent: AgentId) -> Page {
    use x100_client::wording::{Names, wait_on};
    let mut page = Page::new();
    let files = Default::default();
    let n = Names {
        project: c.p,
        daemon: c.d,
        files: &files,
    };
    let Some(a) = c.p.agent(agent) else {
        return page;
    };
    page.heading("Waiting for");
    if a.waits.is_empty() {
        page.text("  Nothing.");
    }
    for w in a.waits.values() {
        page.text(format!(
            "  {} (until {})",
            wait_on(&n, &w.on),
            words::moment(w.deadline)
        ));
    }
    page.blank().heading("Jobs it scheduled");
    let mut any = false;
    for j in c.p.jobs_of(agent) {
        any = true;
        let x100_core::job::JobState::Scheduled { due, .. } = &j.state else {
            continue;
        };
        page.text(format!(
            "  {} at {}",
            x100_client::wording::job(&n, &j.job.does),
            words::moment(*due)
        ));
    }
    if !any {
        page.text("  None.");
    }
    page.blank().heading("In its inbox");
    let waiting = a.inbox.waiting().count();
    if waiting == 0 {
        page.text("  Nothing waiting.");
    }
    for item in a.inbox.waiting() {
        if let Some(i) = a.items.get(&item) {
            page.text(format!("  {}", x100_client::wording::inbox(&n, i)));
        }
    }
    page.blank().heading("Running in its sandbox");
    if a.processes.is_empty() {
        page.text("  Nothing.");
    }
    for (name, pr) in &a.processes {
        page.text(format!("  {} {}", name.0, clip(&pr.command, c.width)));
    }
    page
}
