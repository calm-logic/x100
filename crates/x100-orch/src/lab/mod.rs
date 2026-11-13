//! The lab: trials of reflection's changes, when the project's "trial before commit" setting is
//! on. [`propose`] builds the eval set and writes the change on its trial branch, under the
//! project's writer lock, as part of the CEO's `propose_change`. [`start`] then runs the trial in
//! the background, one trial of a project at a time: every item of the eval set, in each arm,
//! [`x100_lab::RUNS_MIN`] times or more, each by a lab driver in a fresh sandbox ([`run`]). Its
//! verdict is [`x100_lab::rule::rule`]'s. A supported change is merged into main, which is what
//! agents load from; any other is dropped with its branch. Every step is an event, and the CEO
//! hears the verdict through its inbox.

mod calls;
mod item;
mod pack;
mod run;

use std::collections::BTreeMap;
use std::sync::Arc;

use x100_core::action::{Action, Decision};
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, TaskId, TrialId};
use x100_core::inbox::InboxItem;
use x100_core::memory::SkillScope;
use x100_core::money::Money;
use x100_core::party::Actor;
use x100_core::trial::{
    Arm, EvalItem, RunScore, Trial, TrialChange, TrialEvent, TrialState, Verdict,
};
use x100_memory::{Memory, Target};

pub(crate) use calls::LabCalls;
use item::run_item;
use run::Stop;

use crate::Shared;
use crate::ctx::{Events, ProjectCtx, Writer};
use crate::gate;
use crate::ids::new_id;
use crate::settle::deliver;
use crate::store::blocking;
use crate::tools::TrialPlan;

/// What `propose_change` appends for a trial, and what the CEO reads.
pub(crate) struct Proposed {
    pub events: Events,
    pub reply: String,
    /// The trial to run, unless it was decided without a run.
    pub trial: Option<Trial>,
}

fn memory_of(w: &Writer) -> Result<(Memory, x100_store::Git), String> {
    let git = w
        .read(crate::ctx::git_of)
        .ok_or("the project has no code directory, so no memory or skills")?;
    Ok((Memory::new(git.project_dir()), git))
}

/// The repository a change lives in.
fn scope_of(change: &TrialChange) -> SkillScope {
    match change {
        TrialChange::EditSkill { skill, .. } | TrialChange::NewSkill { skill, .. } => skill.scope,
        TrialChange::CompactMemory { agent, .. } => SkillScope::Agent(*agent),
    }
}

/// Every event in the log, oldest first. Blocks.
fn all_events(ctx: &ProjectCtx) -> Result<Vec<Envelope<ProjectEvent>>, String> {
    let mut out = Vec::new();
    let mut after: Option<Seq> = None;
    loop {
        let page = ctx
            .store
            .log
            .read_after(after, 1000)
            .map_err(|e| format!("the log could not be read: {e}"))?;
        let Some(last) = page.last().map(|e| e.seq) else {
            return Ok(out);
        };
        after = Some(last);
        out.extend(page);
    }
}

/// Builds the trial: its eval set, checked and completed from the log, and its change written on
/// its own branch. A compaction that would not leave the memory smaller with every note indexed
/// is decided at once, with no run. Blocks; `w` holds the project's writer lock.
pub(crate) fn propose(
    ctx: &ProjectCtx,
    w: &Writer,
    plan: &TrialPlan,
    by: AgentId,
) -> Result<Proposed, String> {
    let (memory, _git) = memory_of(w)?;
    let past: Vec<TaskId> = plan
        .eval
        .items
        .iter()
        .filter_map(|i| match i {
            EvalItem::PastTask { task, .. } => Some(*task),
            _ => None,
        })
        .collect();
    let mut started_at = BTreeMap::new();
    if !past.is_empty() {
        let events = all_events(ctx)?;
        for task in past {
            let assignee = w.read(|s| Some(s.task(task)?.state.assignment()?.assignee));
            if let Some(found) = assignee.and_then(|a| x100_lab::eval::started_at(&events, task, a))
            {
                started_at.insert(task, found);
            }
        }
    }
    let notes = match &plan.change {
        TrialChange::CompactMemory { agent, .. } => memory
            .notes(*agent)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|n| n.body)
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    };
    let eval = w.read(|state| {
        let facts = x100_lab::eval::Facts {
            state,
            started_at: &started_at,
            notes: &notes,
        };
        x100_lab::eval::build(&plan.change, &plan.eval, &facts)
    })?;
    let id = new_id(w.clock(), TrialId::new);
    let branch = format!("trial/{id}");
    let target = Target::Branch(branch.clone());
    let message = format!("trial {id}");
    let failed = |e: x100_memory::Error| e.to_string();
    let mut discarded = None;
    match &plan.change {
        TrialChange::EditSkill { skill, body } | TrialChange::NewSkill { skill, body } => {
            let new = matches!(plan.change, TrialChange::NewSkill { .. });
            crate::reflect::skill_exists(&memory, skill, new)?;
            memory
                .write_skill(skill, body, &target, &message)
                .map_err(failed)?
                .ok_or("the change says what the skill says already")?;
        }
        TrialChange::CompactMemory { agent, notes } => {
            let changes = memory.compaction(*agent, notes).map_err(failed)?;
            memory
                .change_notes(*agent, &target, &changes, w.clock().now(), &message)
                .map_err(failed)?
                .ok_or("the compaction leaves the notes as they are")?;
            let before = memory.size_on(*agent, &Target::Main).map_err(failed)?;
            let after = memory.size_on(*agent, &target).map_err(failed)?;
            if let Err(why) = x100_lab::rule::compaction(
                crate::reflect::size(before),
                crate::reflect::size(after),
            ) {
                memory
                    .discard(SkillScope::Agent(*agent), &branch)
                    .map_err(failed)?;
                discarded = Some(why);
            }
        }
    }
    let trial = Trial {
        id,
        change: plan.change.clone(),
        eval,
        branch,
    };
    let mut events = vec![
        (Actor::Agent(by), ProjectEvent::TrialProposed(trial.clone())),
        trial_event(id, TrialEvent::Started),
    ];
    if let Some(why) = discarded {
        events.push(trial_event(id, TrialEvent::Scored(Verdict::DoesNotSupport)));
        events.push(trial_event(id, TrialEvent::Discarded { why: why.clone() }));
        let reply = serde_json::json!({ "trial": id, "discarded": why }).to_string();
        return Ok(Proposed {
            events,
            reply,
            trial: None,
        });
    }
    let reply = serde_json::json!({
        "trial": id,
        "under_way": true,
        "message": format!(
            "The trial is under way in the lab: {} runs of each arm. Its verdict reaches your \
             inbox when it is done; you may go on meanwhile.",
            trial.eval.runs_per_arm
        ),
    })
    .to_string();
    Ok(Proposed {
        events,
        reply,
        trial: Some(trial),
    })
}

fn trial_event(trial: TrialId, event: TrialEvent) -> (Actor, ProjectEvent) {
    (Actor::Daemon, ProjectEvent::Trial { trial, event })
}

/// Runs the trial in the background and records its end: the action that proposed it ends, and
/// the CEO is told the verdict, as the decision's result if the builder had approved the call.
pub(crate) fn start(
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
    action: Action,
    trial: Trial,
    held: bool,
) {
    tokio::spawn(async move {
        let _one = ctx.lab.lock().await;
        let (events, words) = run_trial(&shared, &ctx, &trial).await;
        let item = if held {
            x100_gate::tell_agent(action.id, Decision::Approved, Some(words))
        } else {
            InboxItem::Finished {
                action: action.id,
                result: words,
            }
        };
        let id = action.id;
        let writer = Arc::clone(&ctx);
        let done = blocking(move || {
            let w = writer.writer();
            let mut all = events;
            all.push(gate::finished(id, None));
            all.push(deliver(w.clock(), action.agent, item));
            w.append(all).map(drop)
        })
        .await;
        if let Err(e) = done {
            eprintln!("x100: the end of trial {} is not in the log: {e}", trial.id);
        }
    });
}

/// Every run of both arms, then the verdict and what it does to the branch. Returns the events
/// that end the trial and the words the CEO is told.
async fn run_trial(shared: &Arc<Shared>, ctx: &Arc<ProjectCtx>, trial: &Trial) -> (Events, String) {
    let about = ctx.store.log.with_state(|s| pack::about(s, &trial.change));
    let mut runs: Vec<RunScore> = Vec::new();
    let mut dearest = Money::ZERO;
    for run in 1..=trial.eval.runs_per_arm {
        for arm in [Arm::Baseline, Arm::Changed] {
            let mut items = Vec::new();
            for (k, item) in trial.eval.items.iter().enumerate() {
                if !matches!(item, EvalItem::PastTask { .. } | EvalItem::Probe { .. }) {
                    continue;
                }
                if let Some(why) = over_cap(ctx, dearest) {
                    return stop(ctx, trial, &about, why).await;
                }
                match run_item(shared, ctx, trial, arm, run, k, item).await {
                    Ok(score) => {
                        dearest = dearest.max(score.cost);
                        items.push(score);
                    }
                    Err(Stop(why)) => return stop(ctx, trial, &about, why).await,
                }
            }
            let score = x100_lab::rule::run_score(arm, run, &items);
            runs.push(score.clone());
            let event = trial_event(trial.id, TrialEvent::RunFinished(score));
            let ctx2 = Arc::clone(ctx);
            if let Err(e) = blocking(move || ctx2.writer().append(vec![event]).map(drop)).await {
                return stop(
                    ctx,
                    trial,
                    &about,
                    format!("a run could not be recorded: {e}"),
                )
                .await;
            }
        }
    }
    let ruling = x100_lab::rule::rule(&trial.change, &runs, trial.eval.runs_per_arm);
    let (ctx2, trial2) = (Arc::clone(ctx), trial.clone());
    blocking(move || decide(&ctx2, &trial2, ruling, &about)).await
}

/// Whether the next run would pass the lab's cap: the cap is reached, or the dearest run so far
/// would take spending past it.
fn over_cap(ctx: &ProjectCtx, dearest: Money) -> Option<String> {
    ctx.store.log.with_state(|s| {
        let p = s.project()?;
        let cap = p.settings.lab_cap();
        let spent = s.budget().map_or(Money::ZERO, |b| b.lab.spent);
        x100_lab::rule::would_pass_cap(spent, cap, dearest).then(|| {
            format!(
                "the next run would pass the lab's cap of {} micro-dollars: {} spent, and a \
                 run has cost up to {}",
                cap.micros(),
                spent.micros(),
                dearest.micros()
            )
        })
    })
}

/// The trial stops before its verdict: its branch is deleted and the stop recorded.
async fn stop(ctx: &Arc<ProjectCtx>, trial: &Trial, about: &str, why: String) -> (Events, String) {
    let (ctx2, trial2) = (Arc::clone(ctx), trial.clone());
    blocking(move || drop_branch(&ctx2, &trial2)).await;
    let words = format!("The trial of {about} was stopped: {why}. Nothing was changed.");
    (
        vec![trial_event(trial.id, TrialEvent::Stopped { why })],
        words,
    )
}

fn drop_branch(ctx: &ProjectCtx, trial: &Trial) {
    let Some(git) = ctx.git() else { return };
    let memory = Memory::new(git.project_dir());
    if let Err(e) = memory.discard(scope_of(&trial.change), &trial.branch)
        && !matches!(e, x100_memory::Error::NotFound(_))
    {
        eprintln!("x100: could not delete {}: {e}", trial.branch);
    }
}

/// Commits a supported change by merging its branch, or drops it with its branch. Blocks.
fn decide(
    ctx: &ProjectCtx,
    trial: &Trial,
    ruling: x100_lab::rule::Ruling,
    about: &str,
) -> (Events, String) {
    let id = trial.id;
    let mut events = vec![trial_event(id, TrialEvent::Scored(ruling.verdict))];
    let discard = |events: &mut Events, why: String| {
        drop_branch(ctx, trial);
        let words = format!("The trial of {about} dropped the change: {why}.");
        events.push(trial_event(id, TrialEvent::Discarded { why }));
        words
    };
    if ruling.verdict == Verdict::DoesNotSupport {
        let words = discard(&mut events, ruling.why);
        return (events, words);
    }
    let Some(git) = ctx.git() else {
        let words = discard(&mut events, "the project has no code directory".into());
        return (events, words);
    };
    let memory = Memory::new(git.project_dir());
    let now = ctx.clock.now();
    let merged = match &trial.change {
        TrialChange::EditSkill { skill, .. } | TrialChange::NewSkill { skill, .. } => {
            memory.merge(skill.scope, &trial.branch).and_then(|commit| {
                let version = memory.skill(skill, &Target::Main)?.version;
                Ok((
                    commit.clone(),
                    ProjectEvent::SkillCommitted {
                        skill: skill.clone(),
                        version,
                        commit,
                        trial: Some(id),
                    },
                ))
            })
        }
        TrialChange::CompactMemory { agent, .. } => memory
            .merge_notes(*agent, &trial.branch, now)
            .map(|commit| {
                let event = ProjectEvent::MemoryCompacted {
                    agent: *agent,
                    commit: commit.clone(),
                    trial: Some(id),
                };
                (commit, event)
            }),
    };
    match merged {
        Ok((commit, committed)) => {
            events.push(trial_event(id, TrialEvent::Committed { commit }));
            events.push((Actor::Daemon, committed));
            let words = format!("The trial of {about} committed the change: {}.", ruling.why);
            (events, words)
        }
        Err(e) => {
            let why = format!(
                "its runs supported it ({}), but it could not be merged: {e}",
                ruling.why
            );
            let words = discard(&mut events, why);
            (events, words)
        }
    }
}

/// Trials a stopped daemon left running: each is stopped and its branch deleted. Run when a
/// project is attached. Blocks on git.
pub(crate) fn unfinished(state: &ProjectState) -> Events {
    let running: Vec<_> = state
        .trials()
        .filter(|t| matches!(t.state, TrialState::Proposed | TrialState::Running))
        .collect();
    if running.is_empty() {
        return Vec::new();
    }
    let memory = state
        .project()
        .and_then(|p| p.code_dir.as_deref())
        .map(Memory::new);
    let mut out = Vec::new();
    for t in running {
        if let Some(m) = &memory
            && let Err(e) = m.discard(scope_of(&t.trial.change), &t.trial.branch)
            && !matches!(e, x100_memory::Error::NotFound(_))
        {
            eprintln!("x100: could not delete {}: {e}", t.trial.branch);
        }
        if t.state == TrialState::Proposed {
            out.push(trial_event(t.trial.id, TrialEvent::Started));
        }
        let why = "the daemon stopped before the trial finished".to_owned();
        out.push(trial_event(t.trial.id, TrialEvent::Stopped { why }));
    }
    out
}
