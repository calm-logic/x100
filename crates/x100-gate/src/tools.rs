//! Which row of the table a daemon tool call falls under.
//!
//! The match over [`ToolCall`] has no wildcard on purpose: a new tool does not compile until
//! someone decides its row.

use std::time::Duration;

use x100_core::agent::Position;
use x100_core::effort::Effort;
use x100_core::fold::ProjectState;
use x100_core::id::AgentId;
use x100_core::job::Schedule;
use x100_core::tool::{Delegatee, Proposal, ToolCall};

use crate::command::{self, Reading};
use crate::rule::{self, Matched, Rule};

/// A job that repeats faster than this is High risk.
const FAST: Duration = Duration::from_secs(10 * 60);

pub fn rule(
    state: &ProjectState,
    _agent: AgentId,
    call: &ToolCall,
    workspace: Option<&str>,
) -> Matched {
    use ToolCall as T;
    let row: &'static Rule = match call {
        T::Delegate { to, effort, .. } => {
            let reaches_max = *effort == Effort::Max
                || matches!(to, Delegatee::NewWorker { effort: Some(r), .. } if r.max() == Effort::Max);
            if reaches_max {
                &rule::DELEGATE_MAX
            } else {
                &rule::DELEGATE
            }
        }
        // Either kind of job costs money each time it fires.
        T::Schedule { when, .. } => match when {
            Schedule::Once { .. } => &rule::SCHEDULE_ONCE,
            Schedule::Every { every, .. } if every.get() < FAST => &rule::SCHEDULE_FAST,
            Schedule::Every { .. } => &rule::SCHEDULE_REPEATING,
        },
        T::StartProcess { command, .. } => {
            return match command::read(command, workspace) {
                Reading::Matched(m) => m,
                Reading::Unknown | Reading::Ordinary => Matched::of(&rule::START_PROCESS),
            };
        }
        T::FormTeam { .. } => {
            if has_lead(state) {
                &rule::TEAM_CHANGE
            } else {
                &rule::FORM_FIRST
            }
        }
        T::ProposeChange(Proposal::Team { .. }) => &rule::TEAM_CHANGE,
        T::ProposeChange(Proposal::Trial { .. } | Proposal::Correction { .. }) => &rule::ROUTINE,
        T::SetEffort { agent, range } => {
            let raised = state
                .project()
                .and_then(|p| p.team.effort(*agent))
                .is_some_and(|now| range.max() > now.max() || range.min() > now.min());
            if raised {
                &rule::EFFORT_RAISED
            } else {
                &rule::EFFORT_LOWERED
            }
        }
        T::SetBudget { lead, budget } => return budget_rule(state, *lead, *budget),
        T::ReadNote { .. } | T::SearchNotes { .. } | T::SearchLog { .. } => &rule::RECALL,
        T::ListIntegrations {} => &rule::LIST_INTEGRATIONS,
        T::LinkProject { .. } => &rule::LINK_PROJECT,
        // Pushing to a repository x100 has not made yet means making it first.
        T::Deploy { .. } => match state.project() {
            Some(p) if p.homebox.is_some() || p.repository.is_some() => &rule::DEPLOY,
            _ => &rule::DEPLOY_PUBLISH,
        },
        T::Review { .. }
        | T::Report { .. }
        | T::Message { .. }
        | T::Wait { .. }
        | T::CancelJob { .. }
        | T::RaiseTask { .. }
        | T::UpdateTask { .. }
        | T::WriteSpec { .. }
        | T::AddFile { .. }
        | T::SetPreview { .. }
        | T::LoadSkill { .. }
        | T::StopProcess { .. }
        | T::Remember { .. }
        | T::ReadLog { .. }
        | T::ReadMemories { .. }
        | T::SetUpProject { .. } => &rule::ROUTINE,
    };
    Matched::of(row)
}

/// Whether the project has ever had a lead, retired or not: after the first team, every change
/// to the team is held.
fn has_lead(state: &ProjectState) -> bool {
    state.project().is_some_and(|p| {
        p.team
            .members()
            .any(|(_, pos)| matches!(pos, Position::Lead { .. }))
    })
}

fn budget_rule(state: &ProjectState, lead: AgentId, new: x100_core::money::Money) -> Matched {
    let Some(project) = state.project() else {
        return Matched::of(&rule::BUDGET_LOWERED);
    };
    let Some(Position::Lead { budget: now, .. }) = project.team.position(lead) else {
        return Matched::of(&rule::BUDGET_LOWERED);
    };
    if new <= *now {
        return Matched::of(&rule::BUDGET_LOWERED);
    }
    let others: u64 = project
        .team
        .lead_budgets()
        .filter(|(id, _)| *id != lead)
        .map(|(_, b)| b.micros())
        .sum();
    let total = others.saturating_add(new.micros());
    let detail = format!(
        "from {} to {} millionths of a dollar",
        now.micros(),
        new.micros()
    );
    if total > project.settings.budget.micros() {
        Matched::because(&rule::BUDGET_OVER_PROJECT, detail)
    } else {
        Matched::because(&rule::BUDGET_RAISED, detail)
    }
}
