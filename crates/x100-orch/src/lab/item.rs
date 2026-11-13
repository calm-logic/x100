//! One item of one run of one arm: its copy made from a checkpoint, its pack and input, a lab
//! driver to do it, the checks run in its sandbox afterwards, and a judge's mark where the eval
//! set has a rubric.

use std::path::PathBuf;
use std::sync::Arc;

use x100_core::id::{AgentId, TaskId, TrialId};
use x100_core::money::Money;
use x100_core::trial::{Arm, Check, EvalItem, Trial};
use x100_lab::rule::ItemScore;
use x100_memory::Target;

use super::pack;
use super::run::{self, Run, Setup, Stop};
use crate::Shared;
use crate::ctx::ProjectCtx;
use crate::ids::new_id;
use crate::store::blocking;

/// One item of one run of one arm, by a lab driver in a fresh sandbox, with its checks.
pub(super) async fn run_item(
    shared: &Arc<Shared>,
    ctx: &Arc<ProjectCtx>,
    trial: &Trial,
    arm: Arm,
    run: u8,
    k: usize,
    item: &EvalItem,
) -> Result<ItemScore, Stop> {
    let started = ctx.clock.now();
    let target = match arm {
        Arm::Baseline => Target::Main,
        Arm::Changed => Target::Branch(trial.branch.clone()),
    };
    let dir = lab_dir(ctx, trial.id, arm, run, k);
    let (ctx2, trial2, item2, dir2, target2) = (
        Arc::clone(ctx),
        trial.clone(),
        item.clone(),
        dir.clone(),
        target.clone(),
    );
    let prepared = blocking(move || prepare(&ctx2, &trial2, arm, run, &item2, &dir2, &target2))
        .await
        .map_err(Stop)?;
    let scripts: Vec<&str> = checks(trial, |c| match c {
        Check::Script { command } => Some(command.as_str()),
        Check::Rubric { .. } => None,
    });
    let rubrics: Vec<&str> = checks(trial, |c| match c {
        Check::Rubric { text } => Some(text.as_str()),
        Check::Script { .. } => None,
    });
    let past = matches!(item, EvalItem::PastTask { .. });
    let total = if past { 1 + scripts.len() as u32 } else { 1 };
    let setup = Setup {
        purpose: x100_core::driver::Purpose::Lab {
            trial: trial.id,
            arm,
            run,
        },
        agent: prepared.agent,
        target,
        pack: prepared.pack,
        dir: dir.clone(),
    };
    let Some(mut r) = Run::begin(shared, ctx, setup).await? else {
        run::remove(&dir).await;
        return Ok(ItemScore {
            total,
            ..ItemScore::default()
        });
    };
    r.work(shared, ctx, prepared.input).await;
    let reported = r.calls.reported();
    let completed = reported.as_ref().is_some_and(|(done, _)| *done);
    let summary = reported.map(|(_, s)| s).unwrap_or_default();
    let mut passed = u32::from(completed);
    let mut judged = None;
    let mut cost = Money::ZERO;
    match item {
        EvalItem::Probe { answer, .. } => {
            passed = u32::from(completed && x100_lab::eval::answered(answer, &summary));
        }
        _ => {
            for script in &scripts {
                passed += u32::from(r.check(script).await);
            }
            if !rubrics.is_empty() {
                let diff = r.diff().await;
                let mut marks = Vec::new();
                for rubric in &rubrics {
                    let (mark, spent) = judge(
                        shared,
                        ctx,
                        trial,
                        arm,
                        run,
                        prepared.agent,
                        rubric,
                        &summary,
                        &diff,
                    )
                    .await?;
                    marks.push(u32::from(mark.unwrap_or(0)));
                    cost += spent;
                }
                judged = u8::try_from(marks.iter().sum::<u32>() / marks.len() as u32).ok();
            }
        }
    }
    let driven = r.end(shared, ctx).await;
    Ok(ItemScore {
        passed,
        total,
        tool_errors: driven.tool_errors,
        turns: driven.turns,
        duration: ctx.clock.now().since(started),
        cost: driven.cost + cost,
        judged,
    })
}

fn checks<'a>(trial: &'a Trial, pick: impl Fn(&'a Check) -> Option<&'a str>) -> Vec<&'a str> {
    trial
        .eval
        .items
        .iter()
        .filter_map(|i| match i {
            EvalItem::Check(c) => pick(c),
            _ => None,
        })
        .collect()
}

/// A run's own directory, beside the project's scratch directory in the daemon's data, never in
/// the project's code directory.
fn lab_dir(ctx: &ProjectCtx, trial: TrialId, arm: Arm, run: u8, k: usize) -> PathBuf {
    let arm = match arm {
        Arm::Baseline => "base",
        Arm::Changed => "changed",
    };
    let id = trial.to_string();
    let short = &id[id.len().saturating_sub(10)..];
    ctx.store
        .scratch()
        .with_file_name("lab")
        .join(format!("{}-{arm}-{run}-{k}", short.to_ascii_lowercase()))
}

/// What a run starts from.
struct Prepared {
    agent: AgentId,
    pack: String,
    input: String,
}

/// Makes the run's copy from its checkpoint, and its pack and first input. Blocks.
fn prepare(
    ctx: &ProjectCtx,
    trial: &Trial,
    arm: Arm,
    run: u8,
    item: &EvalItem,
    dir: &std::path::Path,
    target: &Target,
) -> Result<Prepared, String> {
    let git = ctx.git().ok_or("the project has no code directory")?;
    let state = ctx.store.log.state();
    let agent = x100_lab::eval::runs_as(&state, &trial.change, item)
        .ok_or("no agent to run this item as")?;
    let (checkpoint, input) = match item {
        EvalItem::PastTask { task, checkpoint } => {
            let t = state.task(*task).ok_or("the past task is gone")?;
            (
                checkpoint.clone(),
                x100_lab::words::task(*task, &t.task.title, &t.task.ask),
            )
        }
        EvalItem::Probe { question, .. } => (
            git.resolve("HEAD").map_err(|e| e.to_string())?,
            x100_lab::words::probe(new_id(&*ctx.clock, TaskId::new), question),
        ),
        EvalItem::Check(_) => return Err("a check is not run on its own".into()),
    };
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("making the lab: {e}"))?;
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|e| format!("clearing an old lab run: {e}"))?;
    }
    git.export(&checkpoint, dir).map_err(|e| {
        format!(
            "the run's copy could not be made from {}: {e}",
            checkpoint.0
        )
    })?;
    let pack = pack::build(
        &state,
        &git,
        &pack::LabPack {
            trial: trial.id,
            change: &trial.change,
            arm,
            run,
            runs: trial.eval.runs_per_arm,
            agent,
            target,
        },
    );
    Ok(Prepared { agent, pack, input })
}

/// A judge's mark for a run against `rubric`, by a lab driver of its own, and what it cost.
#[allow(clippy::too_many_arguments)]
async fn judge(
    shared: &Arc<Shared>,
    ctx: &Arc<ProjectCtx>,
    trial: &Trial,
    arm: Arm,
    run: u8,
    agent: AgentId,
    rubric: &str,
    summary: &str,
    diff: &str,
) -> Result<(Option<u8>, Money), Stop> {
    let dir = lab_dir(ctx, trial.id, arm, run, usize::MAX);
    let made = {
        let dir = dir.clone();
        blocking(move || {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)
        })
        .await
    };
    made.map_err(|e| Stop(format!("making the judge's directory: {e}")))?;
    let about = ctx.store.log.with_state(|s| pack::about(s, &trial.change));
    let pack = format!(
        "{}\nYou are this run's judge: you mark the work against a rubric and change nothing.\n",
        x100_lab::words::head(trial.id, arm, run, trial.eval.runs_per_arm, &about)
    );
    let setup = Setup {
        purpose: x100_core::driver::Purpose::Lab {
            trial: trial.id,
            arm,
            run,
        },
        agent,
        target: Target::Main,
        pack,
        dir: dir.clone(),
    };
    let Some(mut r) = Run::begin(shared, ctx, setup).await? else {
        run::remove(&dir).await;
        return Ok((None, Money::ZERO));
    };
    let task = new_id(&*ctx.clock, TaskId::new);
    r.work(
        shared,
        ctx,
        x100_lab::words::judge(task, rubric, summary, diff),
    )
    .await;
    let driven = r.end(shared, ctx).await;
    let mark = driven.reported.and_then(|(_, s)| x100_lab::words::mark(&s));
    Ok((mark, driven.cost))
}
