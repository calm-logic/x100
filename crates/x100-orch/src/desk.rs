//! Where drivers' tool calls arrive. [`Desk::call`] is the one way in, whatever carries the call:
//! MCP over HTTP for the real harnesses ([`crate::mcp`]), an in-process callback for the fake
//! harness. Both present the driver's id and its token.
//!
//! The desk checks, in one place, that the token is the one it gave that driver and that the log
//! shows the driver as its agent's current one, live. The driver's id only finds its project: the
//! log alone says whether it may act, so a driver that ended, failed or was replaced is refused
//! however slow it is to die, and the check cannot disagree with the record. A refused caller
//! changes nothing in the project's log; the refusal is kept where [`Desk::refusals`] shows it.
//!
//! A call that passes is recorded as an action, put to the gate, planned, carried out with the git
//! work the plan names, and its events appended, all under the project's writer lock.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

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
use x100_store::Git;

use crate::ctx::{Events, ProjectCtx, Writer, git_of};
use crate::gate::{self, Gate};
use crate::ids::new_id;
use crate::store::blocking;
use crate::tools::{self, Ctx, END_THE_TURN, NewCodeDir, SET_UP, Tool};
use crate::{Settings, branches};

/// How many refusals [`Desk::refusals`] keeps, newest last.
const REFUSALS_KEPT: usize = 256;

/// What the desk gave one driver: its token, and the project it works in. Kept for the daemon's
/// life, so a late call from a dead driver is told why it is refused.
struct Pass {
    token: String,
    ctx: Arc<ProjectCtx>,
}

/// A call or listing the desk refused because of who made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub driver: DriverId,
    /// The tool called, or `None` for a listing.
    pub tool: Option<String>,
    pub why: String,
}

pub struct Desk {
    passes: Mutex<HashMap<DriverId, Pass>>,
    refusals: Mutex<VecDeque<Refusal>>,
    /// Where the MCP server listens, once it does. Drivers started before have no address, which
    /// only the fake harness, calling in-process, can do without.
    address: OnceLock<String>,
    gate: Arc<dyn Gate>,
    settings: Settings,
}

impl Desk {
    pub fn new(gate: Arc<dyn Gate>, settings: Settings) -> Arc<Self> {
        Arc::new(Self {
            passes: Mutex::new(HashMap::new()),
            refusals: Mutex::new(VecDeque::new()),
            address: OnceLock::new(),
            gate,
            settings,
        })
    }

    pub(crate) fn listening_at(&self, base: String) {
        let _ = self.address.set(base);
    }

    /// The address and token `driver` reaches the tools with, made as it starts. The token is
    /// worth something only while the log shows the driver as its agent's current one.
    pub(crate) fn issue(&self, driver: DriverId, ctx: Arc<ProjectCtx>) -> ToolEndpoint {
        let token = new_token();
        let url = self
            .address
            .get()
            .map(|base| format!("{base}/drivers/{driver}/mcp"))
            .unwrap_or_default();
        self.passes.lock().expect("desk lock").insert(
            driver,
            Pass {
                token: token.clone(),
                ctx,
            },
        );
        ToolEndpoint { url, token }
    }

    /// The most recent refusals of callers, oldest first.
    pub fn refusals(&self) -> Vec<Refusal> {
        self.refusals
            .lock()
            .expect("desk lock")
            .iter()
            .cloned()
            .collect()
    }

    /// The tools offered to `driver`: those that do something yet, and the CEO's own only to the
    /// CEO's drivers.
    pub(crate) async fn listing(&self, driver: DriverId, token: &str) -> Result<Vec<Tool>, String> {
        let ctx = self.project_of(driver, token, None)?;
        let fenced = blocking(move || {
            ctx.store.log.with_state(|s| {
                let agent = fence(s, driver)?;
                let ceo = s.project().is_some_and(|p| p.team.ceo() == agent);
                Ok(Tool::ALL
                    .into_iter()
                    .filter(|t| t.served() && (ceo || !t.ceo_only()))
                    .collect())
            })
        })
        .await;
        fenced.map_err(|why| self.refuse(driver, None, why))
    }

    /// Carries out one call from `driver`. `Ok` is the result the driver reads; `Err` says why the
    /// call was refused or failed. A call from a driver that may act is an action in the log
    /// either way; one from any other caller changes nothing there.
    pub async fn call(
        &self,
        driver: DriverId,
        token: &str,
        call: ToolCall,
    ) -> Result<String, String> {
        let tool = Tool::of(&call);
        let ctx = self.project_of(driver, token, Some(tool))?;
        let gate = Arc::clone(&self.gate);
        let settings = (self.settings)();
        let done = blocking(move || carry_out(&ctx, &*gate, &settings, driver, call)).await;
        match done {
            Err(Outcome::Fenced(why)) => Err(self.refuse(driver, Some(tool), why)),
            Err(Outcome::Refused(why)) => Err(why),
            Ok(reply) => Ok(reply),
        }
    }

    /// The project of the driver this token was given to.
    fn project_of(
        &self,
        driver: DriverId,
        token: &str,
        tool: Option<Tool>,
    ) -> Result<Arc<ProjectCtx>, String> {
        let found = self
            .passes
            .lock()
            .expect("desk lock")
            .get(&driver)
            .filter(|p| same(&p.token, token))
            .map(|p| Arc::clone(&p.ctx));
        found.ok_or_else(|| {
            let why = format!("this token was not given to {driver}: the call is refused");
            self.refuse(driver, tool, why)
        })
    }

    fn refuse(&self, driver: DriverId, tool: Option<Tool>, why: String) -> String {
        let what = tool.map_or("a listing of the tools", Tool::name);
        eprintln!("x100: refused {what} from {driver}: {why}");
        let mut kept = self.refusals.lock().expect("desk lock");
        if kept.len() == REFUSALS_KEPT {
            kept.pop_front();
        }
        kept.push_back(Refusal {
            driver,
            tool: tool.map(|t| t.name().to_owned()),
            why: why.clone(),
        });
        why
    }
}

#[async_trait]
impl DaemonTools for Desk {
    async fn call(
        &self,
        driver: DriverId,
        endpoint: Option<ToolEndpoint>,
        call: ToolCall,
    ) -> Result<String, String> {
        let token = endpoint.map(|e| e.token).unwrap_or_default();
        Desk::call(self, driver, &token, call).await
    }
}

/// The agent `driver` acts for, if the log shows it as that agent's current driver and live.
/// This is the fencing every call passes.
fn fence(state: &ProjectState, driver: DriverId) -> Result<AgentId, String> {
    let record = state
        .driver(driver)
        .ok_or_else(|| format!("{driver} is not in its project's log"))?;
    let agent = record.driver.agent;
    if !record.state.is_live() {
        return Err(format!(
            "{driver} has ended ({:?}) and acts for nobody: its calls are refused",
            record.state
        ));
    }
    match state.acting_driver(agent) {
        Some(d) if d == driver => Ok(agent),
        Some(d) => Err(format!(
            "{driver} no longer acts for {agent}: {d} does, and its calls are refused"
        )),
        None => Err(format!("{driver} does not act for {agent}")),
    }
}

/// A fresh token: 256 bits from the operating system's generator, as hex.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system's random number generator");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares in time that does not depend on where the strings differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Why a call came to nothing: its caller may not act, or the call itself was refused, which the
/// log records.
enum Outcome {
    Fenced(String),
    Refused(String),
}

fn carry_out(
    ctx: &ProjectCtx,
    gate: &dyn Gate,
    settings: &DaemonSettings,
    driver: DriverId,
    call: ToolCall,
) -> Result<String, Outcome> {
    let store = &ctx.store;
    let file_stored = match &call {
        ToolCall::AddFile { hash, .. } => store.files.contains(hash).unwrap_or(false),
        _ => true,
    };
    // Fenced under the lock, so the driver cannot end between the check and the append.
    let w = ctx.writer();
    let agent = w.read(|s| fence(s, driver)).map_err(Outcome::Fenced)?;
    let clock = w.clock();
    let action = Action {
        id: new_id(clock, ActionId::new),
        agent,
        request: Request::Tool {
            driver,
            call: call.clone(),
        },
    };
    let (ruling, planned) = w.read(|s| {
        let ctx = Ctx {
            agent,
            now: clock.now(),
            settings,
            clock,
            file_stored,
        };
        (gate.rule(s, &action), tools::plan(s, &ctx, &call))
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
    let git = w.read(git_of);
    for agent in &plan.commit {
        if let Some(e) = git.as_ref().and_then(|g| branches::commit(g, *agent).err()) {
            return refuse(&w, head, action.id, e);
        }
    }
    let mut events = plan.events;
    if let Some(new) = &plan.code_dir {
        match make_code_dir(new) {
            Ok(dir) => events.insert(0, (x100_core::party::Actor::Agent(agent), dir)),
            Err(e) => return refuse(&w, head, action.id, e),
        }
    }
    let mut own: Vec<ProjectEvent> = events.iter().map(|(_, e)| e.clone()).collect();
    head.extend(events);
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
                return Ok(answer(&call, &own));
            }
            // Appended before the merge runs, so the task is Promoting while it does.
            append(&w, head)?;
            let outcome = match w.read(git_of) {
                Some(git) => branches::merge(&git, merge.from, merge.into),
                None => Err("the project has no code directory to merge into".into()),
            };
            let mut tail = w.read(|s| tools::merged(s, clock, &merge, merge_action.id, outcome));
            own.extend(tail.iter().map(|(_, e)| e.clone()));
            tail.push(gate::finished(action.id, None));
            append(&w, tail)?;
        }
    }
    if let (ToolCall::WriteSpec { spec, body, .. }, Some(git)) = (&call, &git) {
        write_spec_file(git, *spec, body);
    }
    Ok(answer(&call, &own))
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
) -> Result<String, Outcome> {
    head.push(gate::finished(action, Some(refusal.clone())));
    append(w, head)?;
    Err(Outcome::Refused(refusal))
}

fn append(w: &Writer, events: Events) -> Result<(), Outcome> {
    w.append(events)
        .map(drop)
        .map_err(|e| Outcome::Refused(format!("the log refused this call: {e}")))
}

/// Makes a project's code directory under the projects root, named by its slug, or by the slug and
/// a number when that is taken by a project or anything else, and starts its repository there.
/// The directory is claimed by creating it, so two projects set up at once cannot get the same one.
fn make_code_dir(new: &NewCodeDir) -> Result<ProjectEvent, String> {
    let cannot = |e: &dyn std::fmt::Display| format!("the project's code directory: {e}");
    std::fs::create_dir_all(&new.root).map_err(|e| cannot(&e))?;
    let mut n = 1;
    let dir = loop {
        let name = match n {
            1 => new.slug.clone(),
            _ => format!("{}-{n}", new.slug),
        };
        let dir = new.root.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => break dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(cannot(&e)),
        }
    };
    if let Err(e) = Git::new(dir.clone()).prepare() {
        // Nothing but x100 has used it, since it was made just now.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(cannot(&e));
    }
    Ok(ProjectEvent::CodeDirSet { dir })
}

/// Specs are kept as Markdown in the code directory's `.x100/specs/`, written from the log for
/// people to read. The log is the record, so a file that cannot be written loses nothing.
fn write_spec_file(git: &Git, kind: SpecKind, body: &str) {
    let name = match kind {
        SpecKind::Brd => "BRD.md",
        SpecKind::Prd => "PRD.md",
        SpecKind::Add => "ADD.md",
    };
    let dir = git.project_dir().join(".x100").join("specs");
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(name), body))
    {
        eprintln!("x100: could not write {name}: {e}");
    }
}
