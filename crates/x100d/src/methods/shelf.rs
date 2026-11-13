//! An agent's memories and the project's skills as the repositories hold them, and the builder's
//! way to write a skill. Everything is read and written by the orchestrator through `x100-memory`.

use x100_proto::{
    ListMemories, ListSkills, Memories, MemoryNote, RpcError, SkillInfo, SkillWritten, Skills,
    WriteSkill,
};

use super::{project, refused};
use crate::hub::Hub;

pub async fn memories(hub: &Hub, p: ListMemories) -> Result<Memories, RpcError> {
    project(hub, p.project)?;
    let notes = hub
        .orch
        .memories(p.project, p.agent)
        .await
        .map_err(refused)?;
    Ok(Memories {
        notes: notes
            .into_iter()
            .map(|n| MemoryNote {
                name: n.name,
                commit: n.commit,
                written: n.written,
                sources: n.sources,
                body: n.body,
            })
            .collect(),
    })
}

pub async fn skills(hub: &Hub, p: ListSkills) -> Result<Skills, RpcError> {
    project(hub, p.project)?;
    let skills = hub.orch.skills(p.project, p.scope).await.map_err(refused)?;
    Ok(Skills {
        skills: skills
            .into_iter()
            .map(|s| SkillInfo {
                skill: s.skill,
                version: s.version,
                description: s.description,
                body: s.text,
            })
            .collect(),
    })
}

pub async fn write_skill(hub: &Hub, p: WriteSkill) -> Result<SkillWritten, RpcError> {
    project(hub, p.project)?;
    let (version, commit) = hub
        .orch
        .write_skill(p.project, p.skill, p.text)
        .await
        .map_err(refused)?;
    Ok(SkillWritten { version, commit })
}
