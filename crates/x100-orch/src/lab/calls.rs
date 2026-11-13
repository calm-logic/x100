//! What a lab driver's tool calls do. A lab driver does not act for its agent, so its calls never
//! reach the desk's planner or the project's log: `report` ends its run, `load_skill` and
//! `read_note` read the repositories as its arm has them, and anything else is refused and counts
//! as a tool error.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use x100_core::id::AgentId;
use x100_core::tool::{AgentOutcome, ToolCall};
use x100_memory::{Memory, Target};

use crate::tools::{END_THE_TURN, Tool};

/// One lab driver's run, as its calls see it and leave it.
pub(crate) struct LabCalls {
    pub agent: AgentId,
    pub code_dir: PathBuf,
    /// Main for the baseline, the trial branch for the changed arm.
    pub target: Target,
    reported: Mutex<Option<(bool, String)>>,
    errors: AtomicU32,
}

impl LabCalls {
    pub fn new(agent: AgentId, code_dir: PathBuf, target: Target) -> Self {
        Self {
            agent,
            code_dir,
            target,
            reported: Mutex::new(None),
            errors: AtomicU32::new(0),
        }
    }

    /// Whether it reported, and if it did, whether as completed, with its summary.
    pub fn reported(&self) -> Option<(bool, String)> {
        self.reported.lock().expect("lab calls lock").clone()
    }

    pub fn tool_errors(&self) -> u32 {
        self.errors.load(Ordering::Relaxed)
    }

    /// The tools a lab driver is offered.
    pub fn listing() -> Vec<Tool> {
        vec![Tool::Report, Tool::LoadSkill, Tool::ReadNote]
    }

    /// Answers one call. Blocks on git.
    pub fn answer(&self, call: &ToolCall) -> Result<String, String> {
        let answered = self.answer_(call);
        if answered.is_err() {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        answered
    }

    fn answer_(&self, call: &ToolCall) -> Result<String, String> {
        let memory = Memory::new(&self.code_dir);
        match call {
            ToolCall::Report {
                outcome, summary, ..
            } => {
                let completed = matches!(outcome, AgentOutcome::Completed);
                *self.reported.lock().expect("lab calls lock") = Some((completed, summary.clone()));
                Ok(serde_json::json!({ "reported": true, "next": END_THE_TURN }).to_string())
            }
            ToolCall::LoadSkill { skill } => memory
                .skill_on(skill, &self.target)
                .map(|s| s.text)
                .map_err(|e| e.to_string()),
            ToolCall::ReadNote { note } => memory
                .note_on(self.agent, note, &self.target)
                .map(|n| format!("Note {}:\n\n{}", n.name.0, n.body))
                .map_err(|e| e.to_string()),
            other => Err(format!(
                "`{}` does nothing in the lab: this run is a trial, and nothing it does reaches \
                 the project",
                Tool::of(other).name()
            )),
        }
    }
}
