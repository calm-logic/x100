//! The Settings tab: Ask me about, budgets, which harnesses and models may run, each lead's effort
//! range, credentials, the decision model and reflection. A credential's value is never shown: the
//! daemon never returns one.

use x100_client::words;
use x100_core::agent::Position;
use x100_core::credential::Projects;
use x100_core::trial::{Arm, TrialChange, TrialState};

use super::Ctx;
use crate::page::{BOLD, DIM, GOOD, Page, Setting, Target, WARN, cell, span, spans};

fn on(flag: bool) -> &'static str {
    if flag { "[x]" } else { "[ ]" }
}

pub fn page(c: &Ctx, ig: &crate::integrations::Integrations) -> Page {
    let mut page = Page::new();
    let Some(project) = c.p.project() else {
        return page;
    };
    let s = &project.settings;
    page.styled(
        "Enter changes the selected setting. c adds a credential, v replaces its value, x removes it.",
        DIM,
    );
    page.blank().heading("This project");
    page.row(
        spans(vec![
            span(cell("  Ask me about", 28), Default::default()),
            span(words::ask_me_about(s.ask_me_about), BOLD),
        ]),
        Target::Setting(Setting::AskMeAbout),
    );
    page.row(
        spans(vec![
            span(cell("  Project budget", 28), Default::default()),
            span(words::money(s.budget), BOLD),
        ]),
        Target::Setting(Setting::Budget),
    );
    page.row(
        spans(vec![
            span(cell("  Lab share of the budget", 28), Default::default()),
            span(format!("{}%", s.lab_share.value()), BOLD),
        ]),
        Target::Setting(Setting::LabShare),
    );
    page.row(
        spans(vec![
            span(cell("  Trial before commit", 28), Default::default()),
            span(if s.trial_before_commit { "On" } else { "Off" }, BOLD),
        ]),
        Target::Setting(Setting::TrialBeforeCommit),
    );
    page.blank().heading("Harnesses and models x100 may use");
    for h in c.d.harnesses() {
        for m in &h.models {
            let hm = x100_core::harness::HarnessModel {
                harness: h.id,
                model: m.model.clone(),
            };
            let cooling = if c.d.cooling_off(&hm).is_some() {
                "  (cooling off)"
            } else {
                ""
            };
            page.row(
                spans(vec![
                    span(
                        format!("  {} ", on(s.allowed.contains(&hm))),
                        Default::default(),
                    ),
                    span(
                        format!("{} · {}", words::harness(c.d, h.id), m.model.0),
                        Default::default(),
                    ),
                    span(cooling, WARN),
                ]),
                Target::Setting(Setting::Allow(hm)),
            );
        }
    }
    page.blank().heading("Effort range per lead");
    let mut any = false;
    let mut members: Vec<_> = project.team.members().collect();
    members.sort_by_key(|(id, _)| c.p.agent(*id).map(|a| (a.joined, a.role.clone())));
    for (id, pos) in members {
        if let Position::Lead { effort, budget } = pos {
            any = true;
            page.row(
                spans(vec![
                    span(
                        format!("  {}", cell(&super::role(c.p, id), 14)),
                        Default::default(),
                    ),
                    span(
                        cell(
                            &format!(
                                "{} to {}",
                                words::effort(effort.min()),
                                words::effort(effort.max())
                            ),
                            16,
                        ),
                        BOLD,
                    ),
                    span(format!("   budget {}", words::money(*budget)), DIM),
                ]),
                Target::Lead(id),
            );
        }
    }
    if !any {
        page.text("  No leads yet.");
    }
    page.styled("  r sets the selected lead's range.", DIM);

    page.blank().heading("Credentials (values are never shown)");
    let mut any = false;
    for rec in c.d.credentials() {
        any = true;
        let cr = &rec.credential;
        let hosts = if cr.scope.hosts.is_empty() {
            "no hosts".to_owned()
        } else {
            cr.scope
                .hosts
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        let projects = match &cr.scope.projects {
            Projects::All => "all projects".to_owned(),
            Projects::Only(set) => format!("{} projects", set.len()),
        };
        page.row(
            spans(vec![
                span(format!("  {}", cell(&cr.name, 22)), Default::default()),
                span(format!("{hosts} · {projects}"), DIM),
            ]),
            Target::Credential(cr.reference),
        );
        if !cr.scope.commands.is_empty() {
            page.text(format!(
                "      commands: {}",
                cr.scope
                    .commands
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if !any {
        page.text("  None.");
    }

    page.blank().heading("Decision model");
    let model = c.d.settings().decision_model.as_ref();
    page.row(
        spans(vec![
            span("  ", Default::default()),
            match model {
                Some(m) => span(
                    format!("{} connected", words::decision_provider(m.provider)),
                    GOOD,
                ),
                None => span("None connected. Enter connects one, with an API key.", DIM),
            },
        ]),
        Target::Setting(Setting::DecisionModel),
    );

    page.blank().heading("Reflection");
    page.row(
        spans(vec![span("  Reflect now", BOLD)]),
        Target::Setting(Setting::ReflectNow),
    );
    let mut trials: Vec<_> = c.p.trials().collect();
    trials.sort_by_key(|t| (t.proposed, t.trial.id));
    if trials.is_empty() {
        page.text("  No trials yet.");
    }
    for t in trials {
        page.text(format!(
            "  {} · {}",
            trial_change(c, &t.trial.change),
            trial_state(t.state)
        ));
        let score = |arm: Arm| {
            let runs: Vec<_> = t.runs.iter().filter(|r| r.arm == arm).collect();
            if runs.is_empty() {
                return "no runs".to_owned();
            }
            let passed: u32 = runs.iter().map(|r| r.checks_passed).sum();
            let total: u32 = runs.iter().map(|r| r.checks_total).sum();
            format!("{passed}/{total} checks, {} runs", runs.len())
        };
        if !t.runs.is_empty() {
            page.text(format!(
                "      without: {} · with: {}",
                score(Arm::Baseline),
                score(Arm::Changed)
            ));
        }
        if let Some(why) = &t.why {
            page.paragraph(why, "      ", c.width);
        }
    }
    super::integrations::section(&mut page, c, ig);
    page
}

pub fn trial_change(c: &Ctx, change: &TrialChange) -> String {
    match change {
        TrialChange::EditSkill { skill, .. } => format!("a change to the skill {}", skill.name.0),
        TrialChange::NewSkill { skill, .. } => format!("a new skill, {}", skill.name.0),
        TrialChange::CompactMemory { agent, .. } => {
            format!("compacting {}'s memory", super::role(c.p, *agent))
        }
    }
}

pub fn trial_state(state: TrialState) -> &'static str {
    match state {
        TrialState::Proposed => "Proposed",
        TrialState::Running => "Running",
        TrialState::Scored(x100_core::trial::Verdict::Supports) => "Scored, supports the change",
        TrialState::Scored(_) => "Scored, does not support the change",
        TrialState::Committed => "Committed",
        TrialState::Discarded => "Discarded",
        TrialState::Stopped => "Stopped",
    }
}
