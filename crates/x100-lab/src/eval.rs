//! Eval sets. The CEO proposes one with its change; the daemon checks it against the project and
//! completes it. Three sources, in order of trust:
//!
//! 1. Past tasks from this project's log that ended in an accepted result. Each is replayed from
//!    the checkpoint it started at, which the daemon finds in the log: the newest commit recorded
//!    for the assignee's worktree when it began its first turn on the task. Whatever the CEO
//!    named as the checkpoint is replaced by it.
//! 2. Checks the CEO writes: a script run in each replay's sandbox after the driver is done, or a
//!    rubric a judging driver scores out of 100. A check needs a past task to be run on.
//! 3. For a memory compaction, probe questions whose answers are in the notes before compaction,
//!    asked again of a driver given the memory after it.

use std::collections::{BTreeMap, BTreeSet};

use x100_core::driver::{DriverEvent, Purpose};
use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::git::CommitId;
use x100_core::id::{AgentId, TaskId};
use x100_core::memory::SkillScope;
use x100_core::task::{Outcome, TaskState};
use x100_core::trial::{Check, EvalItem, EvalSet, TrialChange};

use crate::{ITEMS_MAX, RUNS_MAX, RUNS_MIN};

/// The longest script check, in bytes.
const SCRIPT_LIMIT: usize = 2_000;

/// What the daemon read for checking an eval set, besides the folded state.
pub struct Facts<'a> {
    pub state: &'a ProjectState,
    /// Where each past task the eval set names started, as [`started_at`] found it.
    pub started_at: &'a BTreeMap<TaskId, CommitId>,
    /// The bodies of the notes a compaction would replace, for checking that each probe's answer
    /// is in them.
    pub notes: &'a str,
}

/// The eval set the trial runs: the proposed one checked, its past tasks given the checkpoints
/// they started at, and its runs per arm brought within [`RUNS_MIN`] and [`RUNS_MAX`]. `Err` says
/// what to change, in words the CEO can act on.
pub fn build(change: &TrialChange, proposed: &EvalSet, facts: &Facts) -> Result<EvalSet, String> {
    if proposed.items.is_empty() {
        return Err(
            "an eval set needs something to run: a past task, or for a compaction, a \
                    probe question"
                .into(),
        );
    }
    if proposed.items.len() > ITEMS_MAX {
        return Err(format!(
            "an eval set holds at most {ITEMS_MAX} items; this one has {}",
            proposed.items.len()
        ));
    }
    let compaction = matches!(change, TrialChange::CompactMemory { .. });
    let mut items = Vec::new();
    let mut tasks = BTreeSet::new();
    for item in &proposed.items {
        items.push(match item {
            EvalItem::PastTask { task, .. } => {
                if !tasks.insert(*task) {
                    return Err(format!("the past task {task} is named twice"));
                }
                past_task(facts, *task)?
            }
            EvalItem::Check(check) => EvalItem::Check(self::check(check)?),
            EvalItem::Probe { question, answer } => {
                if !compaction {
                    return Err(
                        "probe questions try a memory compaction; a skill change is \
                                tried on past tasks"
                            .into(),
                    );
                }
                probe(facts, question, answer)?
            }
        });
    }
    let has = |f: fn(&EvalItem) -> bool| items.iter().any(f);
    if has(|i| matches!(i, EvalItem::Check(_))) && !has(|i| matches!(i, EvalItem::PastTask { .. }))
    {
        return Err(
            "a check is run on the result of a past task: name an accepted task \
                    for it to be run on"
                .into(),
        );
    }
    if compaction && !has(|i| matches!(i, EvalItem::Probe { .. })) {
        return Err(
            "a compaction is tried with probe questions whose answers are in the notes \
                    now: name at least one"
                .into(),
        );
    }
    if !compaction && !has(|i| matches!(i, EvalItem::PastTask { .. })) {
        return Err(
            "a skill change is tried on past tasks: name at least one accepted task".into(),
        );
    }
    Ok(EvalSet {
        items,
        runs_per_arm: proposed.runs_per_arm.clamp(RUNS_MIN, RUNS_MAX),
    })
}

fn past_task(facts: &Facts, task: TaskId) -> Result<EvalItem, String> {
    let record = facts
        .state
        .task(task)
        .ok_or_else(|| format!("there is no task {task}"))?;
    match &record.state {
        TaskState::Done(_, report) if report.outcome == Outcome::Completed => {}
        other => {
            return Err(format!(
                "task {task} is {}, not accepted: only work that ended in an accepted result \
                 is replayed",
                other.name()
            ));
        }
    }
    let checkpoint = facts.started_at.get(&task).cloned().ok_or_else(|| {
        format!("the log records no checkpoint where task {task} started, so it cannot be replayed")
    })?;
    Ok(EvalItem::PastTask { task, checkpoint })
}

fn check(check: &Check) -> Result<Check, String> {
    match check {
        Check::Script { command } => {
            let command = command.trim();
            if command.is_empty() {
                return Err("a script check needs a command".into());
            }
            if command.len() > SCRIPT_LIMIT {
                return Err(format!(
                    "a script check is at most {SCRIPT_LIMIT} bytes; write a script into the \
                     project and run that"
                ));
            }
            Ok(Check::Script {
                command: command.to_owned(),
            })
        }
        Check::Rubric { text } => {
            if text.trim().is_empty() {
                return Err("a rubric check needs a rubric".into());
            }
            Ok(Check::Rubric {
                text: text.trim().to_owned(),
            })
        }
    }
}

fn probe(facts: &Facts, question: &str, answer: &str) -> Result<EvalItem, String> {
    if question.trim().is_empty() || answer.trim().is_empty() {
        return Err("a probe needs a question and its answer".into());
    }
    if !answered(answer, facts.notes) {
        return Err(format!(
            "the answer to the probe {question:?} is not in the notes before compaction, so \
             asking it again after shows nothing"
        ));
    }
    Ok(EvalItem::Probe {
        question: question.trim().to_owned(),
        answer: answer.trim().to_owned(),
    })
}

/// Whether `text` says `answer`: every word of it, in order, ignoring case and punctuation.
pub fn answered(answer: &str, text: &str) -> bool {
    let words = |s: &str| -> String {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (answer, text) = (words(answer), words(text));
    !answer.is_empty() && format!(" {text} ").contains(&format!(" {answer} "))
}

/// Where `task` started for `assignee`: the newest commit recorded for its worktree, a checkpoint
/// at a turn's end or a refresh from its delegator, when it began its first turn after the task
/// was opened. `None` if the log has no such commit.
pub fn started_at<'a>(
    events: impl IntoIterator<Item = &'a Envelope<ProjectEvent>>,
    task: TaskId,
    assignee: AgentId,
) -> Option<CommitId> {
    let mut drivers = BTreeSet::new();
    let mut last = None;
    let mut opened = false;
    for e in events {
        match &e.event {
            ProjectEvent::DriverStarted(d) if d.agent == assignee && d.purpose == Purpose::Work => {
                drivers.insert(d.id);
            }
            ProjectEvent::Checkpointed { agent, commit }
            | ProjectEvent::BranchRefreshed { agent, commit, .. }
                if *agent == assignee =>
            {
                last = Some(commit.clone());
            }
            ProjectEvent::TaskOpened { task: t, .. } if t.id == task => opened = true,
            ProjectEvent::Driver {
                driver,
                event: DriverEvent::TurnStarted { .. },
            } if opened && drivers.contains(driver) => return last,
            _ => {}
        }
    }
    None
}

/// The agent whose start pack an item is run with: the owner of the memory or the agent's own
/// skill on trial, or for a shared skill, the agent that did the past task.
pub fn runs_as(state: &ProjectState, change: &TrialChange, item: &EvalItem) -> Option<AgentId> {
    let owner = match change {
        TrialChange::CompactMemory { agent, .. } => Some(*agent),
        TrialChange::EditSkill { skill, .. } | TrialChange::NewSkill { skill, .. } => {
            match skill.scope {
                SkillScope::Agent(a) => Some(a),
                SkillScope::Shared => None,
            }
        }
    };
    match item {
        EvalItem::PastTask { task, .. } => {
            owner.or_else(|| Some(state.task(*task)?.state.assignment()?.assignee))
        }
        EvalItem::Probe { .. } => owner,
        EvalItem::Check(_) => None,
    }
}
