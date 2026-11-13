//! A lab driver's start pack: the agent's own role, charter, memory index and skills, read on its
//! arm's branch, under a head that says it is in the lab, and the skill on trial in full as the
//! arm has it, so a run with the change and one without differ in exactly that.

use std::fmt::Write as _;

use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, TrialId};
use x100_core::trial::{Arm, TrialChange};
use x100_memory::{Memory, Target};
use x100_store::Git;

/// What a trial is of, in words.
pub(crate) fn about(state: &ProjectState, change: &TrialChange) -> String {
    match change {
        TrialChange::EditSkill { skill, .. } => format!("a change to the skill {}", skill.name.0),
        TrialChange::NewSkill { skill, .. } => format!("a new skill, {}", skill.name.0),
        TrialChange::CompactMemory { agent, .. } => format!(
            "compacting the memory of {}",
            crate::tools::who(state, x100_core::party::Party::Agent(*agent))
        ),
    }
}

pub(crate) struct LabPack<'a> {
    pub trial: TrialId,
    pub change: &'a TrialChange,
    pub arm: Arm,
    pub run: u8,
    pub runs: u8,
    pub agent: AgentId,
    pub target: &'a Target,
}

/// Builds the pack. Blocks on git.
pub(crate) fn build(state: &ProjectState, git: &Git, p: &LabPack) -> String {
    let mut pack = x100_lab::words::head(p.trial, p.arm, p.run, p.runs, &about(state, p.change));
    if let Some(a) = state.agent(p.agent) {
        let _ = write!(
            pack,
            "\nROLE: {}\nYour id: {}\n\nCHARTER:\n{}\n",
            a.role, p.agent, a.charter
        );
    }
    let mem = Memory::new(git.project_dir());
    let (always, hidden) = crate::pack::skill_rules(state, p.agent);
    let index = mem.index_on(p.agent, p.target).unwrap_or_else(|e| {
        eprintln!(
            "x100: could not read {}'s memory for a lab run: {e}",
            p.agent
        );
        String::new()
    });
    if index.trim().is_empty() {
        pack.push_str("\nYOUR MEMORY: no notes yet.\n");
    } else {
        let _ = write!(pack, "\nYOUR MEMORY:\n{index}");
    }
    match mem.skills_section_on(p.agent, always, hidden, p.target) {
        Ok(skills) if !skills.is_empty() => {
            let _ = write!(pack, "\n{skills}");
        }
        Ok(_) => {}
        Err(e) => eprintln!("x100: could not read the skills for a lab run: {e}"),
    }
    if let TrialChange::EditSkill { skill, .. } | TrialChange::NewSkill { skill, .. } = p.change {
        let text = mem.skill_on(skill, p.target).ok().map(|s| s.text);
        pack.push_str(&x100_lab::words::skill_on_trial(
            &skill.name.0,
            text.as_deref(),
        ));
    }
    pack
}
