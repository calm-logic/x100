//! The CEO's reflection tools: `read_log` and `propose_change`. The planner checks what the folded
//! state decides. Reading the log, writing a note or a skill, and building an eval set need the
//! store and the repositories, so the plan names that work and the desk does it: a correction or
//! a change committed directly through [`crate::recall`], a trial through [`crate::lab`].
//!
//! The project's "trial before commit" setting decides what a skill change or a compaction does:
//! off, it is committed at once, as a commit and an event; on, it is tried in the lab first. A
//! correction of a note against the log is never put on trial. A team change is held for the
//! builder by the gate, whatever the "Ask me about" level, and carried out here as events once
//! the builder approves.

use x100_core::agent::{AgentEvent, AgentState, Position};
use x100_core::event::{ProjectEvent, Seq};
use x100_core::id::AgentId;
use x100_core::ledger::Budgets;
use x100_core::memory::{NoteName, SkillScope};
use x100_core::party::Actor;
use x100_core::team::{TeamChange, TeamOp};
use x100_core::tool::Proposal;
use x100_core::trial::{EvalSet, TrialChange};

use super::memory::MemoryPlan;
use super::{At, Plan, Refusal};
use crate::ids::new_id;

/// The most events one `read_log` call returns.
pub(crate) const READ_LOG_MAX: u64 = 200;

/// A change to try in the lab, which the desk hands to [`crate::lab`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrialPlan {
    pub change: TrialChange,
    pub eval: EvalSet,
}

fn memory(memory: MemoryPlan) -> Plan {
    Plan {
        memory: Some(memory),
        ..Plan::default()
    }
}

fn need_code_dir(at: &At) -> Result<(), Refusal> {
    match at.project.code_dir {
        Some(_) => Ok(()),
        None => Err("memory and skills start when the project is set up".into()),
    }
}

pub(crate) fn read_log(at: &At, from: Seq, to: Option<Seq>) -> Result<Plan, Refusal> {
    let last = at.state.seq().map_or(0, |s| s.0);
    if from.0 == 0 || from.0 > last {
        return Err(format!(
            "the log runs from event 1 to event {last}: start within it"
        ));
    }
    if let Some(to) = to
        && to.0 < from.0
    {
        return Err("`to` comes before `from`".into());
    }
    Ok(memory(MemoryPlan::ReadLog {
        from,
        to: to.map_or(last, |t| t.0.min(last)),
    }))
}

pub(crate) fn propose(at: &At, proposal: &Proposal) -> Result<Plan, Refusal> {
    match proposal {
        Proposal::Correction {
            agent,
            note,
            body,
            sources,
        } => correction(at, *agent, note, body, sources),
        Proposal::Trial { change, eval } => {
            need_code_dir(at)?;
            check_change(at, change)?;
            if at.project.settings.trial_before_commit {
                Ok(Plan {
                    trial: Some(TrialPlan {
                        change: change.clone(),
                        eval: eval.clone(),
                    }),
                    ..Plan::default()
                })
            } else {
                Ok(memory(MemoryPlan::Commit {
                    change: change.clone(),
                }))
            }
        }
        Proposal::Team {
            changes, telemetry, ..
        } => team(at, changes, telemetry),
    }
}

fn correction(
    at: &At,
    agent: AgentId,
    note: &NoteName,
    body: &str,
    sources: &[Seq],
) -> Result<Plan, Refusal> {
    need_code_dir(at)?;
    at.live_agent(agent)?;
    let note = x100_memory::Memory::note_name(note).map_err(|e| e.to_string())?;
    if body.trim().is_empty() {
        return Err("the corrected note is empty".into());
    }
    if sources.is_empty() {
        return Err(
            "a correction cites the events that show the note wrong: name them in `sources`".into(),
        );
    }
    let last = at.state.seq().map_or(0, |s| s.0);
    if let Some(bad) = sources.iter().find(|s| s.0 == 0 || s.0 > last) {
        return Err(format!("event {} is not in the log", bad.0));
    }
    Ok(memory(MemoryPlan::Correct {
        agent,
        note,
        body: body.to_owned(),
        sources: sources.to_vec(),
    }))
}

/// What the state alone can say of a change. Whether a skill exists is the repository's to say.
fn check_change(at: &At, change: &TrialChange) -> Result<(), Refusal> {
    match change {
        TrialChange::EditSkill { skill, body } | TrialChange::NewSkill { skill, body } => {
            if let SkillScope::Agent(owner) = skill.scope {
                at.live_agent(owner)?;
            }
            if body.trim().is_empty() {
                return Err("the skill is empty".into());
            }
        }
        TrialChange::CompactMemory { agent, notes } => {
            at.live_agent(*agent)?;
            if notes.is_empty() {
                return Err(
                    "a compaction names every note the memory keeps: it cannot keep none".into(),
                );
            }
        }
    }
    Ok(())
}

/// A team change: each step checked against the team as the steps before it leave it, and the
/// events that carry it out. They are appended only once the builder approves.
fn team(at: &At, changes: &[TeamChange], telemetry: &str) -> Result<Plan, Refusal> {
    if changes.is_empty() {
        return Err("a team change needs at least one change".into());
    }
    if telemetry.trim().is_empty() {
        return Err(
            "a team change carries the telemetry that prompted it: what the log shows, with \
             numbers"
                .into(),
        );
    }
    let me = Actor::Agent(at.id());
    let mut team = at.project.team.clone();
    let mut events = Vec::new();
    let mut outlines = Vec::new();
    let agent_event = |agent, event| (me, ProjectEvent::Agent { agent, event });
    for change in changes {
        let mut step = |op: TeamOp, event: (Actor, ProjectEvent)| -> Result<(), Refusal> {
            team = team.apply(&op).map_err(|e| e.to_string())?;
            events.push(event);
            Ok(())
        };
        match change {
            TeamChange::AddLead(lead) => {
                if lead.role.trim().is_empty() {
                    return Err("every lead needs a role".into());
                }
                let id = new_id(at.ctx.clock, AgentId::new);
                let position = Position::Lead {
                    effort: lead.effort,
                    budget: lead.budget,
                };
                let appearance = at.child_look(&outlines);
                outlines.push(appearance.outline);
                let added = x100_core::agent::Agent {
                    id,
                    role: lead.role.clone(),
                    charter: lead.charter.clone(),
                    position: position.clone(),
                    appearance,
                };
                step(
                    TeamOp::Join {
                        agent: id,
                        position,
                    },
                    (me, ProjectEvent::AgentAdded(added)),
                )?;
            }
            TeamChange::Promote {
                worker,
                effort,
                budget,
            } => step(
                TeamOp::Promote {
                    agent: *worker,
                    effort: *effort,
                    budget: *budget,
                },
                agent_event(
                    *worker,
                    AgentEvent::Promoted {
                        effort: *effort,
                        budget: *budget,
                    },
                ),
            )?,
            TeamChange::Retire { agent } => {
                retirable(at, *agent)?;
                step(
                    TeamOp::Retire { agent: *agent },
                    agent_event(*agent, AgentEvent::Retired),
                )?;
            }
            TeamChange::Fold { lead, into } => {
                retirable(at, *lead)?;
                let workers: Vec<AgentId> = at.state.children(*lead);
                for w in workers {
                    step(
                        TeamOp::Move {
                            agent: w,
                            under: *into,
                        },
                        agent_event(w, AgentEvent::Moved { under: *into }),
                    )?;
                }
                step(
                    TeamOp::Retire { agent: *lead },
                    agent_event(*lead, AgentEvent::Retired),
                )?;
            }
        }
    }
    Budgets::in_force(&at.project.settings, &team)
        .check_leads()
        .map_err(|e| e.to_string())?;
    Ok(Plan {
        events,
        ..Plan::default()
    })
}

/// An agent is retired only between tasks: one at work would lose its task.
fn retirable(at: &At, agent: AgentId) -> Result<(), Refusal> {
    let a = at.live_agent(agent)?;
    if at.project.team.ceo() == agent {
        return Err("the CEO is not retired".into());
    }
    if !a.open_tasks.is_empty() || !matches!(a.state, AgentState::Idle | AgentState::Planned) {
        return Err(format!(
            "{agent} is at work: it can be retired once its tasks are done"
        ));
    }
    Ok(())
}
