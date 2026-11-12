//! Requests out through the proxy and commands a harness's permission hook stops, at the gate.
//!
//! Each is an action. An approved one the agent has not used yet lets exactly the same request
//! through once ("allowed once", rebuilt from the log, so it survives a restart). Otherwise the
//! rules rule on it, or, for what they do not cover, the decision model, asked outside the
//! project's lock since a call takes its time. Either way the requester is answered at once: a
//! held action never holds up a driver.

use std::sync::Arc;

use x100_core::action::{Action, Request};
use x100_core::id::{ActionId, AgentId, DriverId};
use x100_drivers::{Decision, PermissionRequest};
use x100_gate::{AllowedOnce, Asked, Pending, Ruling, Verdict};

use crate::Shared;
use crate::ctx::ProjectCtx;
use crate::ids::new_id;
use crate::store::blocking;
use crate::{classify, gate};

/// The gate's answer to the requester.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Admitted {
    /// Let it through. The action is ended by whoever sees it end: the proxy's report for a
    /// request, at once for a command.
    Go(ActionId),
    /// Refuse it, saying why: held for the builder, or not the gate's to allow.
    Refused {
        action: Option<ActionId>,
        why: String,
    },
}

enum First {
    Done(Admitted),
    Ask(Box<(Action, Pending)>),
}

/// Puts `request` of `agent` to the gate and records what it decided.
pub(crate) async fn admit(
    shared: &Arc<Shared>,
    ctx: &Arc<ProjectCtx>,
    agent: AgentId,
    request: Request,
) -> Admitted {
    let first = {
        let (shared, ctx) = (Arc::clone(shared), Arc::clone(ctx));
        blocking(move || rules(&shared, &ctx, agent, request)).await
    };
    let (action, pending) = match first {
        Ok(First::Done(admitted)) => return admitted,
        Ok(First::Ask(asked)) => *asked,
        Err(e) => return unrecorded(e),
    };
    let (asked, mut events) = match action.request {
        Request::Egress { driver, .. } | Request::Command { driver, .. } => {
            classify::classify(shared, ctx, pending.question(), driver).await
        }
        // The rules settle these; one that ever reached here is held.
        Request::Tool { .. } | Request::Merge { .. } => {
            let why = "only requests out and commands are put to the decision model".into();
            (Asked::NoAnswer { call: None, why }, Vec::new())
        }
    };
    let ruling = pending.resolve(asked, (shared.settings)().decision_threshold);
    events.extend(gate::ruled(action.id, &ruling));
    let admitted = answer_for(&action, &ruling);
    if let Admitted::Go(id) = admitted
        && matches!(action.request, Request::Command { .. })
    {
        events.push(gate::finished(id, None));
    }
    let ctx = Arc::clone(ctx);
    match blocking(move || ctx.writer().append(events)).await {
        Ok(_) => admitted,
        Err(e) => unrecorded(e.to_string()),
    }
}

/// The answer to a command a harness's permission hook stopped: allowed, or denied with why.
pub(crate) async fn hook(
    (shared, ctx, agent, driver): (&Arc<Shared>, &Arc<ProjectCtx>, AgentId, DriverId),
    request: &PermissionRequest,
) -> Decision {
    let command = match request.input.get("command").and_then(|c| c.as_str()) {
        Some(command) => command.to_owned(),
        None => format!("{} {}", request.tool, request.input),
    };
    match admit(shared, ctx, agent, Request::Command { driver, command }).await {
        Admitted::Go(_) => Decision::Allow,
        Admitted::Refused { why, .. } => Decision::Deny { reason: why },
    }
}

/// Under the lock: a grant the agent holds for exactly this, or the rules' ruling, recorded. What
/// the rules leave to the classifier is recorded as proposed and handed back.
fn rules(
    shared: &Shared,
    ctx: &ProjectCtx,
    agent: AgentId,
    request: Request,
) -> Result<First, String> {
    let w = ctx.writer();
    if w.read(|s| s.agent(agent).is_none()) {
        return Ok(First::Done(Admitted::Refused {
            action: None,
            why: "x100 refused this: the agent it came from is not on the team".into(),
        }));
    }
    let command = matches!(request, Request::Command { .. });
    if let Some(granted) = w.read(|s| grant(shared, s, agent, &request)) {
        if command {
            w.append(vec![gate::finished(granted, None)])
                .map_err(|e| e.to_string())?;
        } else {
            // The proxy's report ends it. Until then no second request may use it.
            shared
                .in_flight
                .lock()
                .expect("in-flight lock")
                .insert(granted);
        }
        return Ok(First::Done(Admitted::Go(granted)));
    }
    let action = Action {
        id: new_id(w.clock(), ActionId::new),
        agent,
        request,
    };
    let workspace = w.read(|s| gate::workspace(s, agent));
    match w.read(|s| x100_gate::assess(s, &action, workspace.as_deref())) {
        Verdict::Ruled(ruling) => {
            let mut events = gate::proposed(&action, &ruling);
            let admitted = answer_for(&action, &ruling);
            if command && admitted == Admitted::Go(action.id) {
                events.push(gate::finished(action.id, None));
            }
            w.append(events).map_err(|e| e.to_string())?;
            Ok(First::Done(admitted))
        }
        Verdict::Ask(pending) => {
            w.append(vec![gate::proposal(&action)])
                .map_err(|e| e.to_string())?;
            Ok(First::Ask(Box::new((action, pending))))
        }
    }
}

/// The approved action of `agent` that lets exactly `request` through once, and that no request
/// in flight is using.
fn grant(
    shared: &Shared,
    state: &x100_core::fold::ProjectState,
    agent: AgentId,
    request: &Request,
) -> Option<ActionId> {
    let in_flight = shared.in_flight.lock().expect("in-flight lock");
    let mut once = AllowedOnce::from_state(state);
    // Each take uses a grant up, so this ends.
    while let Some(granted) = once.take(agent, request) {
        if !in_flight.contains(&granted) {
            return Some(granted);
        }
    }
    None
}

fn answer_for(action: &Action, ruling: &Ruling) -> Admitted {
    match ruling {
        Ruling::Allow(_) => Admitted::Go(action.id),
        Ruling::Hold(c) => Admitted::Refused {
            action: Some(action.id),
            why: gate::held_words(action.id, &c.why, gate::on_approve(&action.request)),
        },
    }
}

/// The log would not take the gate's record, so nothing goes through.
fn unrecorded(error: String) -> Admitted {
    eprintln!("x100: an action could not be recorded, so it is refused: {error}");
    Admitted::Refused {
        action: None,
        why: "x100 refused this: it could not be recorded".into(),
    }
}
