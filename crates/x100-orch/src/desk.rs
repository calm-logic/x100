//! Where drivers' tool calls arrive. [`Desk::handle`] is the one entry point, whatever carries the
//! call: the fake harness's callback today, MCP for the real harnesses with M6b. It finds the
//! calling driver's agent, records the call as an action, asks the gate, plans the call, does the
//! git work the plan names, and appends what happened.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use x100_core::action::{Action, Request};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId, DriverId};
use x100_core::settings::DaemonSettings;
use x100_core::spec::SpecKind;
use x100_core::tool::ToolCall;
use x100_drivers::ToolEndpoint;
use x100_drivers::fake::DaemonTools;

use crate::ctx::{Events, ProjectCtx, Writer};
use crate::gate::{self, Gate};
use crate::ids::new_id;
use crate::store::blocking;
use crate::tools::{self, Ctx};
use crate::{Settings, branches};

#[derive(Clone)]
struct Seat {
    ctx: Arc<ProjectCtx>,
    agent: AgentId,
}

pub struct Desk {
    seats: Mutex<HashMap<DriverId, Seat>>,
    gate: Arc<dyn Gate>,
    settings: Settings,
}

impl Desk {
    pub fn new(gate: Arc<dyn Gate>, settings: Settings) -> Arc<Self> {
        Arc::new(Self {
            seats: Mutex::new(HashMap::new()),
            gate,
            settings,
        })
    }

    pub(crate) fn seat(&self, driver: DriverId, ctx: Arc<ProjectCtx>, agent: AgentId) {
        self.seats
            .lock()
            .expect("desk lock")
            .insert(driver, Seat { ctx, agent });
    }

    pub(crate) fn unseat(&self, driver: DriverId) {
        self.seats.lock().expect("desk lock").remove(&driver);
    }

    /// Carries out one call from `driver`. `Ok` is the result the driver reads; `Err` says why the
    /// call was refused or failed. Either way the call is an action in the log.
    pub async fn handle(&self, driver: DriverId, call: ToolCall) -> Result<String, String> {
        let seat = self
            .seats
            .lock()
            .expect("desk lock")
            .get(&driver)
            .cloned()
            .ok_or_else(|| format!("{driver} is not a live driver of this daemon"))?;
        let gate = Arc::clone(&self.gate);
        let settings = (self.settings)();
        blocking(move || carry_out(&seat, &*gate, &settings, driver, call)).await
    }
}

#[async_trait]
impl DaemonTools for Desk {
    async fn call(
        &self,
        driver: DriverId,
        _endpoint: Option<ToolEndpoint>,
        call: ToolCall,
    ) -> Result<String, String> {
        self.handle(driver, call).await
    }
}

fn carry_out(
    seat: &Seat,
    gate: &dyn Gate,
    settings: &DaemonSettings,
    driver: DriverId,
    call: ToolCall,
) -> Result<String, String> {
    let store = &seat.ctx.store;
    let file_stored = match &call {
        ToolCall::AddFile { hash, .. } => store.files.contains(hash).unwrap_or(false),
        _ => true,
    };
    let w = seat.ctx.writer();
    let clock = w.clock();
    let action = Action {
        id: new_id(clock, ActionId::new),
        agent: seat.agent,
        request: Request::Tool {
            driver,
            call: call.clone(),
        },
    };
    let (ruling, planned) = w.read(|s| {
        let ctx = Ctx {
            agent: seat.agent,
            now: clock.now(),
            settings,
            clock,
            file_stored,
        };
        let planned = acting(s, seat.agent, driver).and_then(|()| tools::plan(s, &ctx, &call));
        (gate.rule(s, &action), planned)
    });
    let mut head = gate::proposed(&action, &ruling);
    if ruling.hold {
        append(&w, head)?;
        return Ok(r#"{"held":"for the builder's decision; wait on it or carry on"}"#.into());
    }
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => return refuse(&w, head, action.id, refusal),
    };
    for agent in &plan.commit {
        if let Err(e) = branches::commit(store, *agent) {
            return refuse(&w, head, action.id, e);
        }
    }
    let mut own: Vec<ProjectEvent> = plan.events.iter().map(|(_, e)| e.clone()).collect();
    head.extend(plan.events);
    match plan.merge {
        None => {
            head.push(gate::finished(action.id, None));
            append(&w, head)?;
        }
        Some(merge) => {
            let merge_action = Action {
                id: new_id(clock, ActionId::new),
                agent: merge.by,
                request: Request::Merge { task: merge.task },
            };
            let ruling = w.read(|s| gate.rule(s, &merge_action));
            head.extend(gate::proposed(&merge_action, &ruling));
            if ruling.hold {
                head.push(gate::finished(action.id, None));
                append(&w, head)?;
                return Ok(tools::reply(&call, &own));
            }
            // Appended before the merge runs, so the task is Promoting while it does.
            append(&w, head)?;
            let outcome = branches::merge(store, merge.from, merge.into);
            let mut tail = w.read(|s| tools::merged(s, clock, &merge, merge_action.id, outcome));
            own.extend(tail.iter().map(|(_, e)| e.clone()));
            tail.push(gate::finished(action.id, None));
            append(&w, tail)?;
        }
    }
    if let ToolCall::WriteSpec { spec, body, .. } = &call {
        write_spec_file(&seat.ctx, *spec, body);
    }
    Ok(tools::reply(&call, &own))
}

/// Only the agent's current driver acts for it. Handoff (M9) fences the rest off properly; this
/// is the part that needs nothing more than the log.
fn acting(state: &ProjectState, agent: AgentId, driver: DriverId) -> Result<(), String> {
    if state.acting_driver(agent) == Some(driver) {
        Ok(())
    } else {
        Err(format!("{driver} no longer acts for {agent}"))
    }
}

fn refuse(
    w: &Writer,
    mut head: Events,
    action: ActionId,
    refusal: String,
) -> Result<String, String> {
    head.push(gate::finished(action, Some(refusal.clone())));
    append(w, head)?;
    Err(refusal)
}

fn append(w: &Writer, events: Events) -> Result<(), String> {
    w.append(events)
        .map(drop)
        .map_err(|e| format!("the log refused this call: {e}"))
}

/// Specs are kept as Markdown in the project's `.x100/specs/`, written from the log for people
/// to read. The log is the record, so a file that cannot be written loses nothing.
fn write_spec_file(ctx: &ProjectCtx, kind: SpecKind, body: &str) {
    let name = match kind {
        SpecKind::Brd => "BRD.md",
        SpecKind::Prd => "PRD.md",
        SpecKind::Add => "ADD.md",
    };
    let dir = ctx.store.git.project_dir().join(".x100").join("specs");
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(name), body))
    {
        eprintln!("x100: could not write {name}: {e}");
    }
}
