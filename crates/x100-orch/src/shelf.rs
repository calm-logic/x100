//! The builder's view of the memory and skill repositories, and the one way the builder changes a
//! skill. Every file is read and written through `x100-memory`. A change is a commit on main and a
//! `SkillCommitted` event in the same step, with no trial behind it: the builder's own edit is not
//! put on trial. Reflection's changes will come through the same two steps.

use std::path::PathBuf;

use x100_core::event::ProjectEvent;
use x100_core::git::CommitId;
use x100_core::id::AgentId;
use x100_core::memory::{SkillRef, SkillScope, SkillVersion};
use x100_core::party::Actor;
use x100_memory::{Memory, Note, Skill, Target};

use crate::builder::Refused;
use crate::ctx::ProjectCtx;

fn memory(ctx: &ProjectCtx) -> Result<Memory, Refused> {
    let dir: Option<PathBuf> = ctx.git().map(|g| g.project_dir().to_owned());
    dir.map(|d| Memory::new(&d)).ok_or_else(|| {
        Refused::WrongState("the project has no code directory yet, so no memory or skills".into())
    })
}

fn failed(e: x100_memory::Error) -> Refused {
    match e {
        x100_memory::Error::NotFound(m) => Refused::NotFound(m),
        x100_memory::Error::Invalid(m) => Refused::Rejected(m),
        other => Refused::WrongState(other.to_string()),
    }
}

/// An agent's notes on main, newest first. A project that has no code directory has none.
pub(crate) fn memories(ctx: &ProjectCtx, agent: AgentId) -> Result<Vec<Note>, Refused> {
    if ctx.store.log.with_state(|s| s.agent(agent).is_none()) {
        return Err(Refused::NotFound(format!("no agent {agent}")));
    }
    match memory(ctx) {
        Ok(m) => m.notes(agent).map_err(failed),
        Err(_) => Ok(Vec::new()),
    }
}

/// The skills of one scope on main, or the shared ones and every agent's own for `None`.
pub(crate) fn skills(ctx: &ProjectCtx, scope: Option<SkillScope>) -> Result<Vec<Skill>, Refused> {
    let Ok(mem) = memory(ctx) else {
        return Ok(Vec::new());
    };
    let scopes: Vec<SkillScope> = match scope {
        Some(s) => vec![s],
        None => {
            let agents: Vec<AgentId> = ctx
                .store
                .log
                .with_state(|s| s.agents().map(|a| a.id).collect());
            std::iter::once(SkillScope::Shared)
                .chain(agents.into_iter().map(SkillScope::Agent))
                .collect()
        }
    };
    let mut out = Vec::new();
    for scope in scopes {
        out.extend(mem.skills(scope, &Target::Main).map_err(failed)?);
    }
    Ok(out)
}

/// Writes a skill to main as the builder, raising its version, and records the change.
pub(crate) fn write_skill(
    ctx: &ProjectCtx,
    skill: SkillRef,
    text: String,
) -> Result<(SkillVersion, CommitId), Refused> {
    if let SkillScope::Agent(a) = skill.scope
        && ctx.store.log.with_state(|s| s.agent(a).is_none())
    {
        return Err(Refused::NotFound(format!("no agent {a}")));
    }
    let mem = memory(ctx)?;
    crate::recall::ensure_shared(ctx);
    let w = ctx.writer();
    let message = format!("skill {}: written by the builder", skill.name.0);
    let (version, commit) = mem
        .write_skill(&skill, &text, &Target::Main, &message)
        .map_err(failed)?
        .ok_or_else(|| Refused::Rejected(format!("{} already says that", skill.name.0)))?;
    w.append(vec![(
        Actor::Builder,
        ProjectEvent::SkillCommitted {
            skill,
            version,
            commit: commit.clone(),
            trial: None,
        },
    )])?;
    Ok((version, commit))
}
