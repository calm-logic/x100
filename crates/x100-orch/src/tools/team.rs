//! The CEO's `form_team`, `set_effort` and `set_budget`, and the tools that write the project's
//! records: `write_spec`, `add_file` and `set_preview`.

use std::collections::BTreeSet;

use x100_core::agent::{Agent, AgentEvent, Position};
use x100_core::effort::EffortRange;
use x100_core::event::ProjectEvent;
use x100_core::file::{ContentHash, FileRef};
use x100_core::id::{AgentId, FileId};
use x100_core::ledger::Budgets;
use x100_core::money::Money;
use x100_core::party::{Actor, Party};
use x100_core::spec::{SpecKind, SpecRevision, SpecStatus};
use x100_core::team::{LeadSpec, TeamOp};

use super::{At, Plan, Refusal};
use crate::ids::new_id;

fn ceo_only(at: &At, what: &str) -> Result<(), Refusal> {
    if at.is_ceo() {
        Ok(())
    } else {
        Err(format!("only the CEO {what}"))
    }
}

fn one(at: &At, event: ProjectEvent) -> Plan {
    Plan {
        events: vec![(Actor::Agent(at.id()), event)],
        ..Plan::default()
    }
}

pub(super) fn form(at: &At, leads: &[LeadSpec]) -> Result<Plan, Refusal> {
    let team = &at.project.team;
    if team
        .members()
        .any(|(_, p)| matches!(p, Position::Lead { .. }))
    {
        return Err(
            "the team is formed already: in v1 every later change to it is proposed to the \
             builder, which arrives with M14"
                .into(),
        );
    }
    if leads.is_empty() {
        return Err("a team needs at least one lead; the CEO may also work alone".into());
    }
    let mut roles = BTreeSet::new();
    for lead in leads {
        let role = lead.role.trim().to_lowercase();
        if role.is_empty() {
            return Err("every lead needs a role".into());
        }
        if !roles.insert(role) {
            return Err(format!("two leads are named {}", lead.role));
        }
    }
    let mut events = Vec::new();
    let mut shaped = team.clone();
    for lead in leads {
        let id = new_id(at.ctx.clock, AgentId::new);
        let position = Position::Lead {
            effort: lead.effort,
            budget: lead.budget,
        };
        shaped = shaped
            .apply(&TeamOp::Join {
                agent: id,
                position: position.clone(),
            })
            .map_err(|e| e.to_string())?;
        events.push((
            Actor::Agent(at.id()),
            ProjectEvent::AgentAdded(Agent {
                id,
                role: lead.role.clone(),
                charter: lead.charter.clone(),
                position,
            }),
        ));
    }
    Budgets::in_force(&at.project.settings, &shaped)
        .check_leads()
        .map_err(|e| e.to_string())?;
    Ok(Plan {
        events,
        ..Plan::default()
    })
}

pub(super) fn set_effort(at: &At, agent: AgentId, range: EffortRange) -> Result<Plan, Refusal> {
    at.live_agent(agent)?;
    let event = ProjectEvent::Agent {
        agent,
        event: AgentEvent::EffortSet(range),
    };
    Ok(one(at, event))
}

pub(super) fn set_budget(at: &At, lead: AgentId, budget: Money) -> Result<Plan, Refusal> {
    at.live_agent(lead)?;
    let shaped = at
        .project
        .team
        .apply(&TeamOp::SetBudget {
            agent: lead,
            budget,
        })
        .map_err(|e| e.to_string())?;
    Budgets::in_force(&at.project.settings, &shaped)
        .check_leads()
        .map_err(|e| e.to_string())?;
    let event = ProjectEvent::Agent {
        agent: lead,
        event: AgentEvent::BudgetSet(budget),
    };
    Ok(one(at, event))
}

/// The CEO writes the BRD. The PRD and the ADD are the Product and Tech leads' or the CEO's, and
/// since the CEO names roles freely, any lead may write them.
pub(super) fn write_spec(
    at: &At,
    kind: SpecKind,
    status: SpecStatus,
    body: &str,
) -> Result<Plan, Refusal> {
    let lead = matches!(
        at.project.team.position(at.id()),
        Some(Position::Lead { .. })
    );
    match kind {
        SpecKind::Brd => ceo_only(at, "writes the BRD")?,
        SpecKind::Prd | SpecKind::Add if !at.is_ceo() && !lead => {
            return Err("the PRD and the ADD are written by the CEO or a lead".into());
        }
        _ => {}
    }
    let revision = at.state.spec(kind).map_or(1, |s| s.spec.revision + 1);
    let spec = SpecRevision {
        kind,
        author: at.id(),
        revision,
        status,
        body: body.to_owned(),
    };
    Ok(one(at, ProjectEvent::SpecWritten(spec)))
}

pub(super) fn add_file(
    at: &At,
    name: &str,
    media_type: &str,
    size: u64,
    hash: &ContentHash,
) -> Result<Plan, Refusal> {
    if !at.ctx.file_stored {
        return Err(format!(
            "no file {} is in the project's file store: put the bytes there first",
            hash.0
        ));
    }
    if name.trim().is_empty() {
        return Err("a file needs a name".into());
    }
    let file = FileRef {
        id: new_id(at.ctx.clock, FileId::new),
        name: name.to_owned(),
        media_type: media_type.to_owned(),
        size,
        hash: hash.clone(),
        added_by: Party::Agent(at.id()),
        attached_to: None,
    };
    Ok(one(at, ProjectEvent::FileAdded(file)))
}

pub(super) fn set_preview(at: &At, url: &str) -> Result<Plan, Refusal> {
    if url.trim().is_empty() {
        return Err("a preview needs an address".into());
    }
    Ok(one(
        at,
        ProjectEvent::PreviewSet {
            url: url.to_owned(),
        },
    ))
}
