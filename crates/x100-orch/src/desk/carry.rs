//! Carrying out a call the desk admitted: fenced on the log under the project's writer lock, put
//! to the gate, planned, and carried out with the git and sandbox work its plan names.

use std::sync::Arc;

use x100_core::action::{Action, Decision, Request};
use x100_core::driver::Purpose;
use x100_core::event::ProjectEvent;
use x100_core::id::{ActionId, DriverId};
use x100_core::settings::DaemonSettings;
use x100_core::tool::ToolCall;
use x100_gate::Ruling;
use x100_integrations::Integrations;
use x100_vault::Requester;

use super::fence;
use crate::ctx::{Events, ProjectCtx, Writer, git_of};
use crate::gate;
use crate::ids::new_id;
use crate::sandbox::Sandboxes;
use crate::settle::deliver;
use crate::tools::{self, Ctx, END_THE_TURN, SET_UP, Tool};
use crate::{Shared, branches};

/// Why a call came to nothing: its caller may not act, or the call itself was refused, which the
/// log records.
pub(super) enum Outcome {
    Fenced(String),
    Refused(String),
}

/// What carrying out a call may use besides the project.
pub(super) struct Using {
    pub sandboxes: Option<Arc<Sandboxes>>,
    pub integrations: Option<Arc<Integrations>>,
    pub settings: DaemonSettings,
    pub homebox: crate::integrate::Prefetched,
    pub shared: Option<Arc<Shared>>,
}

/// The call's action, once it is in the log, and how the call ended.
pub(super) fn carry_out(
    ctx: &Arc<ProjectCtx>,
    using: &Using,
    driver: DriverId,
    call: ToolCall,
) -> (Option<ActionId>, Result<String, Outcome>) {
    // Fenced under the lock, so the driver cannot end between the check and the append.
    let w = ctx.writer();
    let agent = match w.read(|s| fence(s, driver)) {
        Ok(agent) => agent,
        Err(why) => return (None, Err(Outcome::Fenced(why))),
    };
    let action = Action {
        id: new_id(w.clock(), ActionId::new),
        agent,
        request: Request::Tool { driver, call },
    };
    let ruling = w.read(|s| gate::rule(s, &action));
    let planned = plan(ctx, &w, using, &action);
    // A call the tools refuse is refused whatever the gate says, so the builder is never asked
    // about what would not be done. It reaches the plan, which is what allowed means here.
    let done = if let Err(refusal) = planned {
        let allowed = Ruling::Allow(ruling.classification().clone());
        refuse(
            &w,
            gate::proposed(&action, &allowed),
            action.id,
            refusal,
            None,
        )
    } else if ruling.is_held() {
        let head = gate::proposed(&action, &ruling);
        append(&w, head).map(|()| {
            let words = gate::held_words(
                action.id,
                &ruling.classification().why,
                gate::on_approve(&action.request),
            );
            serde_json::json!({ "held": action.id, "message": words }).to_string()
        })
    } else {
        let head = gate::proposed(&action, &ruling);
        execute(ctx, &w, using, &action, head, planned, false)
    };
    (Some(action.id), done)
}

/// The tool call's plan on the state as it is now.
pub(super) fn plan(
    ctx: &ProjectCtx,
    w: &Writer,
    using: &Using,
    action: &Action,
) -> Result<tools::Plan, String> {
    let Request::Tool { call, .. } = &action.request else {
        unreachable!("only a tool call is planned by the desk")
    };
    let file_stored = match call {
        ToolCall::AddFile { hash, .. } => ctx.store.files.contains(hash).unwrap_or(false),
        _ => true,
    };
    let (clock, dice) = (w.clock(), w.dice());
    w.read(|s| {
        let ctx = Ctx {
            agent: action.agent,
            now: clock.now(),
            settings: &using.settings,
            clock,
            dice,
            file_stored,
            homebox: using.homebox.as_ref(),
        };
        tools::plan(s, &ctx, call)
    })
}

/// Carries out a tool call the gate allowed or the builder approved, by its plan on the state as
/// it is now: does the git and sandbox work the plan names, and appends `head` with what happened,
/// ending the action. With `tell`, the agent is also given the builder's approval with the
/// call's reply, or why it failed, in the append that ends the action.
pub(super) fn execute(
    ctx: &Arc<ProjectCtx>,
    w: &Writer,
    using: &Using,
    action: &Action,
    mut head: Events,
    planned: Result<tools::Plan, String>,
    tell: bool,
) -> Result<String, Outcome> {
    let Request::Tool { driver, call } = &action.request else {
        unreachable!("only a tool call is carried out by the desk")
    };
    let (agent, driver) = (action.agent, *driver);
    let clock = w.clock();
    let told = |result: Result<&str, &str>| {
        tell.then(|| {
            let result = match result {
                Ok(reply) => reply.to_owned(),
                Err(why) => format!("It could not be carried out: {why}"),
            };
            deliver(
                clock,
                agent,
                x100_gate::tell_agent(action.id, Decision::Approved, Some(result)),
            )
        })
    };
    let fail = |head: Events, refusal: String| {
        let tell = told(Err(&refusal));
        refuse(w, head, action.id, refusal, tell)
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => return fail(head, refusal),
    };
    let git = w.read(git_of);
    for agent in &plan.commit {
        if let Some(e) = git.as_ref().and_then(|g| branches::commit(g, *agent).err()) {
            return fail(head, e);
        }
    }
    let mut reply_override = None;
    if let Some(process) = &plan.process {
        let requester = Requester {
            project: ctx.id,
            agent,
            driver,
            purpose: Purpose::Work,
        };
        match crate::workdir::carry_out_process(using.sandboxes.as_deref(), requester, process) {
            Ok(reply) => reply_override = reply,
            Err(e) => return fail(head, e),
        }
    }
    let mut memory_events = Vec::new();
    if let Some(memory) = &plan.memory {
        let Some(code_dir) = w.read(|s| s.project().and_then(|p| p.code_dir.clone())) else {
            return fail(
                head,
                "the project has no code directory to keep memory in".into(),
            );
        };
        match crate::recall::run(&ctx.store, &code_dir, agent, memory, clock.now()) {
            Ok(done) => {
                reply_override = Some(done.reply);
                memory_events = done.events;
            }
            Err(e) => return fail(head, e),
        }
    }
    if let Some(job) = plan.deploy {
        let Some(integrations) = using.integrations.clone() else {
            return fail(head, "this daemon has no integrations".into());
        };
        // The action stays open until the deploy is done; then it ends, and the agent is told.
        append(w, head)?;
        let reply = crate::integrate::started(action);
        crate::integrate::start(
            Arc::clone(ctx),
            integrations,
            using.settings.clone(),
            action.clone(),
            job,
            tell,
        );
        return Ok(reply);
    }
    if let Some(trial) = &plan.trial {
        let Some(shared) = using.shared.clone() else {
            return fail(head, "this daemon has no lab".into());
        };
        let proposed = match crate::lab::propose(ctx, w, trial, agent) {
            Ok(p) => p,
            Err(e) => return fail(head, e),
        };
        head.extend(proposed.events);
        let reply = proposed.reply;
        match proposed.trial {
            // The action stays open until the trial is done; then it ends, and the agent is told.
            Some(trial) => {
                append(w, head)?;
                crate::lab::start(shared, Arc::clone(ctx), action.clone(), trial, tell);
            }
            None => {
                head.push(gate::finished(action.id, None));
                head.extend(told(Ok(&reply)));
                append(w, head)?;
            }
        }
        return Ok(reply);
    }
    if let Some(reply) = plan.reply {
        reply_override = Some(reply);
    }
    let mut events = plan.events;
    events.extend(
        memory_events
            .into_iter()
            .map(|e| (x100_core::party::Actor::Agent(agent), e)),
    );
    if let Some(new) = &plan.code_dir {
        match crate::workdir::make_code_dir(new) {
            Ok(dir) => events.insert(0, (x100_core::party::Actor::Agent(agent), dir)),
            Err(e) => return fail(head, e),
        }
    }
    let mut own: Vec<ProjectEvent> = events.iter().map(|(_, e)| e.clone()).collect();
    head.extend(events);
    let reply = match plan.merge {
        None => {
            let reply = reply_override.unwrap_or_else(|| answer(call, &own));
            head.push(gate::finished(action.id, None));
            head.extend(told(Ok(&reply)));
            append(w, head)?;
            reply
        }
        Some(merge) => {
            let merge_action = Action {
                id: new_id(clock, ActionId::new),
                agent: merge.by,
                request: Request::Merge { task: merge.task },
            };
            let ruling = w.read(|s| gate::rule(s, &merge_action));
            head.extend(gate::proposed(&merge_action, &ruling));
            if ruling.is_held() {
                let reply = answer(call, &own);
                head.push(gate::finished(action.id, None));
                head.extend(told(Ok(&reply)));
                append(w, head)?;
                return Ok(reply);
            }
            // Appended before the merge runs, so the task is Promoting while it does.
            append(w, head)?;
            let outcome = match w.read(git_of) {
                Some(git) => branches::merge(&git, merge.from, merge.into),
                None => Err("the project has no code directory to merge into".into()),
            };
            let mut tail = w.read(|s| tools::merged(s, clock, &merge, merge_action.id, outcome));
            own.extend(tail.iter().map(|(_, e)| e.clone()));
            let reply = reply_override.unwrap_or_else(|| answer(call, &own));
            tail.push(gate::finished(action.id, None));
            tail.extend(told(Ok(&reply)));
            append(w, tail)?;
            reply
        }
    };
    if let (ToolCall::WriteSpec { spec, body, .. }, Some(git)) = (call, &git) {
        crate::workdir::write_spec_file(git, *spec, body);
    }
    Ok(reply)
}

/// The reply to a call that was carried out. After `wait` or `report`, or the call that sets the
/// project up, it also tells the model its part in the turn is over, which the start pack's record
/// of the call leaves out.
fn answer(call: &ToolCall, events: &[ProjectEvent]) -> String {
    let reply = tools::reply(call, events);
    let next = if events.iter().any(|e| matches!(e, ProjectEvent::SetUp(_))) {
        SET_UP
    } else if Tool::of(call).ends_turn() {
        END_THE_TURN
    } else {
        return reply;
    };
    match serde_json::from_str::<serde_json::Value>(&reply) {
        Ok(serde_json::Value::Object(mut m)) => {
            m.insert("next".into(), next.into());
            serde_json::Value::Object(m).to_string()
        }
        _ => format!("{reply}\n{next}"),
    }
}

fn refuse(
    w: &Writer,
    mut head: Events,
    action: ActionId,
    refusal: String,
    tell: Option<(x100_core::party::Actor, ProjectEvent)>,
) -> Result<String, Outcome> {
    head.push(gate::finished(action, Some(refusal.clone())));
    head.extend(tell);
    append(w, head)?;
    Err(Outcome::Refused(refusal))
}

fn append(w: &Writer, events: Events) -> Result<(), Outcome> {
    w.append(events)
        .map(drop)
        .map_err(|e| Outcome::Refused(format!("the log refused this call: {e}")))
}
