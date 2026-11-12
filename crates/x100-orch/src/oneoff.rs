//! One-off calls: a single turn with no tools, through the cheapest allowed harness and model at
//! its lowest effort, for an answer that needs a model but no agent. The gate's classifier is the
//! one caller so far. A one-off is not a driver and belongs to no agent: it is recorded as a
//! `OneOffCalled` event and a charge to the acting agent's payer.
//!
//! It runs in the acting agent's sandbox, with a route of its own on the proxy that reaches its
//! provider and nothing else, so a question that quotes the agent's own text cannot lead it
//! anywhere the agent could not already go.

use std::sync::Arc;
use std::time::Duration;

use x100_core::effort::Effort;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessModel, ModelName, OneOffCall};
use x100_core::id::{AgentId, DriverId, HarnessId, OneOffCallId};
use x100_core::ledger::{Charge, ChargeSource};
use x100_core::party::Actor;
use x100_drivers::{DriverOutput, DriverSpec, Harness, SpawnHook, Spent, StallPolicy};
use x100_gate::{Answer, Question};

use crate::Shared;
use crate::ctx::{Events, ProjectCtx, git_of};
use crate::ids::new_id;
use crate::spec;
use crate::store::blocking;

/// How long the answer may take. The action waits meanwhile; past this it is held.
const ANSWER_WITHIN: Duration = Duration::from_secs(120);

/// What the model is sent as its turn: the question itself is its start pack.
const ASK: &str = "Label the action described in your instructions, with exactly one line.";

struct Pick {
    harness: HarnessId,
    program: Arc<dyn Harness>,
    model: ModelName,
    effort: Effort,
}

/// The classifier's answer to `question`, and the events that record the call and its charge.
/// `None` when no harness could be asked or it gave no answer, which holds the action.
pub(crate) async fn classify(
    shared: &Arc<Shared>,
    ctx: &Arc<ProjectCtx>,
    question: &Question,
) -> (Option<Answer>, Events) {
    let agent = question.agent;
    let Some(pick) = pick(shared, ctx).await else {
        eprintln!(
            "x100: no harness this project allows can classify action {}: it is held",
            question.action
        );
        return (None, Vec::new());
    };
    let clock = Arc::clone(&ctx.clock);
    let call = new_id(&*clock, OneOffCallId::new);
    let driver = new_id(&*clock, DriverId::new);
    shared
        .one_offs
        .lock()
        .expect("one-offs lock")
        .insert(driver);
    let asked = ask(shared, ctx, agent, driver, &pick, question).await;
    shared
        .one_offs
        .lock()
        .expect("one-offs lock")
        .remove(&driver);
    let _ = shared.processes.forget(driver);
    let Some((text, spent)) = asked
        .map_err(|e| eprintln!("x100: a one-off call: {e}"))
        .ok()
    else {
        return (None, Vec::new());
    };
    let mut events: Events = vec![(
        Actor::Daemon,
        ProjectEvent::OneOffCalled(OneOffCall {
            id: call,
            purpose: question.purpose(),
            harness: pick.harness,
            model: pick.model.clone(),
        }),
    )];
    let payer = read(ctx, move |s| s.project().and_then(|p| p.team.payer(agent))).await;
    if let (Some(spent), Some(payer)) = (spent, payer) {
        events.push((
            Actor::Daemon,
            ProjectEvent::Charged(Charge {
                payer,
                source: ChargeSource::OneOff { call },
                usage: spent.usage,
                cost: spent.cost,
                basis: spent.basis,
            }),
        ));
    }
    (text.map(|text| Answer { call, text }), events)
}

/// The cheapest model the project allows on a harness this daemon can start, at its lowest
/// effort, ties broken by harness and model so the choice is the same on every run.
async fn pick(shared: &Arc<Shared>, ctx: &Arc<ProjectCtx>) -> Option<Pick> {
    let allowed: Vec<HarnessModel> = read(ctx, |s| {
        s.project()
            .map(|p| p.settings.allowed.iter().cloned().collect())
            .unwrap_or_default()
    })
    .await;
    let best = shared.daemon.with_state(|d| {
        allowed
            .iter()
            .filter(|hm| shared.harness(hm.harness).is_some())
            .filter_map(|hm| {
                let offer = d
                    .harness(hm.harness)?
                    .models
                    .iter()
                    .find(|m| m.model == hm.model)?;
                let (effort, price) = offer.levels.iter().min_by_key(|(e, _)| *e)?;
                let cost = x100_router::blended(price);
                Some((cost, hm.harness, hm.model.clone(), *effort))
            })
            .min()
    })?;
    let (_, harness, model, effort) = best;
    Some(Pick {
        harness,
        program: shared.harness(harness)?,
        model,
        effort,
    })
}

/// Starts the program in the agent's sandbox, gives it one turn, and stops it. Returns the text
/// it wrote, if its turn completed, and what it spent.
async fn ask(
    shared: &Arc<Shared>,
    ctx: &Arc<ProjectCtx>,
    agent: AgentId,
    driver: DriverId,
    pick: &Pick,
    question: &Question,
) -> Result<(Option<String>, Option<Spent>), String> {
    let dir = read(ctx, move |s| git_of(s).map(|g| g.worktree_path(agent)))
        .await
        .unwrap_or_else(|| ctx.store.scratch());
    let harness = &*pick.program;
    // Held to the end: it holds the call's way out.
    let placed = spec::place(shared, ctx, agent, driver, harness, &dir).await?;
    let mut env = placed.env.clone();
    env.extend(spec::api_key(shared, ctx, harness));
    let login = shared.daemon.with_state(|d| {
        d.harness(pick.harness)
            .and_then(|h| h.login.as_ref())
            .map(|l| std::path::PathBuf::from(&l.0))
    });
    let processes = Arc::clone(&shared.processes);
    let spec = DriverSpec {
        driver,
        model: pick.model.clone(),
        effort: pick.effort,
        dir,
        state: placed.state.clone(),
        wrap: placed.wrap.clone(),
        expose: placed.expose.clone(),
        tmp: placed.tmp.clone(),
        start_pack: question.prompt.clone(),
        tools: None,
        env,
        stall: StallPolicy::from_settings(&(shared.settings)()),
        permissions: spec::PERMISSIONS,
        login,
        on_spawn: SpawnHook::new(move |pid| {
            if let Err(e) = processes.record(driver, pid) {
                eprintln!("x100: could not write down one-off call {driver}'s process: {e}");
            }
        }),
    };
    let mut handle = harness
        .start(spec)
        .await
        .map_err(|e| format!("the classifier's program did not start: {e}"))?;
    let control = handle.control();
    let mut text = Vec::new();
    let mut spent = None;
    let answered = tokio::time::timeout(ANSWER_WITHIN, async {
        control
            .turn(ASK)
            .await
            .map_err(|e| format!("the classifier's program took no turn: {e}"))?;
        loop {
            match handle.next().await {
                Some(DriverOutput::Text(t)) => text.push(t),
                Some(DriverOutput::Spent(s)) => spent = Some(s),
                Some(DriverOutput::TurnEnded(_)) => return Ok(true),
                Some(DriverOutput::Failed(f)) => return Err(format!("it failed: {f:?}")),
                // It was given no tools, so nothing it asks about is allowed.
                Some(DriverOutput::PermissionRequest(r)) => {
                    let deny = x100_drivers::Decision::Deny {
                        reason: "a one-off call runs nothing".into(),
                    };
                    let _ = control.answer(r.id, deny).await;
                }
                Some(_) => {}
                None => return Ok(false),
            }
        }
    })
    .await;
    control.stop().await;
    drop(placed);
    let completed = match answered {
        Ok(Ok(completed)) => completed,
        Ok(Err(e)) => return Err(e),
        Err(_) => false,
    };
    let text = completed.then(|| text.join("\n").trim().to_owned());
    Ok((text, spent))
}

async fn read<T: Send + 'static>(
    ctx: &Arc<ProjectCtx>,
    f: impl FnOnce(&ProjectState) -> T + Send + 'static,
) -> T {
    let ctx = Arc::clone(ctx);
    blocking(move || ctx.store.log.with_state(f)).await
}

/// A one-off's driver id is no driver of the log's: its routes are for its provider alone.
pub(crate) fn is_one_off(shared: &Shared, driver: DriverId) -> bool {
    shared
        .one_offs
        .lock()
        .expect("one-offs lock")
        .contains(&driver)
}
