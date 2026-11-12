//! Trials: reflection's tests. A change to a skill or a memory is tried on an eval set with and
//! without the change, in the lab, and committed only when the trial supports it.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::git::CommitId;
use crate::id::{AgentId, TaskId, TrialId};
use crate::machine::{MachineKind, Rejected};
use crate::memory::{NoteName, SkillRef};
use crate::money::Money;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Trial {
    pub id: TrialId,
    pub change: TrialChange,
    pub eval: EvalSet,
    /// The branch of the memory or skills repository the change lives on while it is on trial.
    pub branch: String,
}

/// A change reflection may make, if a trial supports it. Team changes are not put on trial in v1.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialChange {
    EditSkill {
        skill: SkillRef,
        body: String,
    },
    NewSkill {
        skill: SkillRef,
        body: String,
    },
    CompactMemory {
        agent: AgentId,
        notes: Vec<(NoteName, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EvalSet {
    pub items: Vec<EvalItem>,
    /// Each arm runs more than once, because results vary between runs.
    pub runs_per_arm: u8,
}

/// Something to run and how to score it, in order of trust.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalItem {
    /// A past task that ended in an accepted result, replayed from the checkpoint it started at and
    /// scored by the checks that passed then.
    PastTask { task: TaskId, checkpoint: CommitId },
    /// A check the CEO wrote for this trial.
    Check(Check),
    /// For a memory compaction: a question whose answer is in the notes before compaction.
    Probe { question: String, answer: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Script {
        command: String,
    },
    /// For a judging model, where no script can decide.
    Rubric {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// As things are.
    Baseline,
    /// With the change.
    Changed,
}

/// The scores of one run of one arm.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunScore {
    pub arm: Arm,
    pub run: u8,
    pub checks_passed: u32,
    pub checks_total: u32,
    pub tool_errors: u32,
    pub turns: u32,
    pub duration: Duration,
    pub cost: Money,
    /// The judge's score out of 100, only where a rubric was used.
    pub judged: Option<u8>,
}

/// Whether the trial supports the change: no worse on any check, and better on at least one measure
/// by the set margin, across the repeated runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Supports,
    DoesNotSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialState {
    Proposed,
    Running,
    Scored(Verdict),
    /// The change was merged into the main branch that agents load from.
    Committed,
    /// The branch was deleted. The trial is kept.
    Discarded,
    /// Going on would have passed the lab's cap.
    Stopped,
}

impl TrialState {
    pub const INITIAL: TrialState = TrialState::Proposed;

    pub fn name(self) -> &'static str {
        match self {
            TrialState::Proposed => "Proposed",
            TrialState::Running => "Running",
            TrialState::Scored(Verdict::Supports) => "Scored(Supports)",
            TrialState::Scored(Verdict::DoesNotSupport) => "Scored(DoesNotSupport)",
            TrialState::Committed => "Committed",
            TrialState::Discarded => "Discarded",
            TrialState::Stopped => "Stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialEvent {
    Started,
    RunFinished(RunScore),
    Scored(Verdict),
    Committed { commit: CommitId },
    Discarded,
    Stopped,
}

impl TrialEvent {
    pub fn name(&self) -> &'static str {
        match self {
            TrialEvent::Started => "Started",
            TrialEvent::RunFinished(_) => "RunFinished",
            TrialEvent::Scored(_) => "Scored",
            TrialEvent::Committed { .. } => "Committed",
            TrialEvent::Discarded => "Discarded",
            TrialEvent::Stopped => "Stopped",
        }
    }
}

/// The trial machine. Proposed → Running → Scored → Committed or Discarded. Running → Stopped when
/// the lab's cap would be passed. A trial that supports its change can only be committed, and one
/// that does not can only be discarded, so the rule for committing is decided once, when it is
/// scored.
pub fn next(state: &TrialState, event: &TrialEvent) -> Result<TrialState, Rejected> {
    use TrialEvent as E;
    use TrialState as S;
    let next = match (state, event) {
        (S::Proposed, E::Started) => Some(S::Running),
        (S::Running, E::RunFinished(_)) => Some(S::Running),
        (S::Running, E::Scored(verdict)) => Some(S::Scored(*verdict)),
        (S::Running, E::Stopped) => Some(S::Stopped),
        (S::Scored(Verdict::Supports), E::Committed { .. }) => Some(S::Committed),
        (S::Scored(Verdict::DoesNotSupport), E::Discarded) => Some(S::Discarded),
        _ => None,
    };
    next.ok_or(Rejected {
        machine: MachineKind::Trial,
        state: state.name(),
        event: event.name(),
    })
}
