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
//! A call that passes is recorded as an action and put to the gate. One the gate allows is planned,
//! carried out with the git and sandbox work the plan names, and its events appended, all under
//! the project's writer lock. One it holds is answered at once, and carried out the same way, as
//! the agent, if the builder approves it ([`Desk::approved`]).
//! Every credential value in a call is replaced by its reference first, so no message, report,
//! spec, note or file name a driver sends holds one.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use x100_core::action::{Action, Decision, Request};
use x100_core::driver::Purpose;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId, DriverId};
use x100_core::settings::DaemonSettings;
use x100_core::spec::SpecKind;
use x100_core::tool::ToolCall;
use x100_drivers::ToolEndpoint;
use x100_drivers::fake::DaemonTools;
use x100_gate::Ruling;
use x100_integrations::Integrations;
use x100_store::Git;
use x100_vault::{Redactor, Requester};

use crate::ctx::{Events, ProjectCtx, Writer, git_of};
use crate::gate;
use crate::ids::new_id;
use crate::sandbox::Sandboxes;
use crate::settle::deliver;
use crate::store::blocking;
use crate::tools::{self, Ctx, END_THE_TURN, NewCodeDir, ProcessPlan, SET_UP, Tool};
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
    settings: Settings,
    redactor: Redactor,
    /// Where `start_process` runs, set once the orchestrator is made.
    sandboxes: OnceLock<Arc<Sandboxes>>,
    /// What the integration tools call, set once by the daemon. Without it they are refused.
    integrations: OnceLock<Arc<Integrations>>,
}

impl Desk {
    pub fn new(settings: Settings, redactor: Redactor) -> Arc<Self> {
        Arc::new(Self {
            passes: Mutex::new(HashMap::new()),
            refusals: Mutex::new(VecDeque::new()),
            address: OnceLock::new(),
            settings,
            redactor,
            sandboxes: OnceLock::new(),
            integrations: OnceLock::new(),
        })
    }

    /// Gives the integration tools their way to Homebox and GitHub. Set once.
    pub fn attach_integrations(&self, integrations: Arc<Integrations>) {
        let _ = self.integrations.set(integrations);
    }

    /// What carrying out `call` may use, read now: the settings, and what Homebox says if the
    /// call reads it.
    async fn using(&self, call: &ToolCall) -> Using {
        let settings = (self.settings)();
        let integrations = self.integrations.get().cloned();
        let homebox = crate::integrate::prefetch(integrations.as_deref(), call, &settings).await;
        Using {
            sandboxes: self.sandboxes.get().cloned(),
            integrations,
            settings,
            homebox,
        }
    }

    pub(crate) fn listening_at(&self, base: String) {
        let _ = self.address.set(base);
    }

    pub(crate) fn attach(&self, sandboxes: Arc<Sandboxes>) {
        let _ = self.sandboxes.set(sandboxes);
    }

    /// The tool server's address on loopback, which every sandbox maps in.
    pub(crate) fn tools_addr(&self) -> Option<SocketAddr> {
        self.address
            .get()
            .and_then(|url| url.strip_prefix("http://"))
            .and_then(|a| a.parse().ok())
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
        let call = crate::redact::value(&self.redactor, &call)
            .ok_or("the call could not be cleared of credential values, so it is refused")?;
        let using = self.using(&call).await;
        let done = blocking(move || carry_out(&ctx, &using, driver, call)).await;
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

    /// Carries out a held tool call the builder approved, as its agent, under the lock `w` holds,
    /// whichever driver the agent has now. `head` (the approval) is appended with what happened,
    /// and so is the decision for the agent's inbox, with the call's reply or why it failed.
    /// Blocks.
    pub(crate) fn approved(
        &self,
        ctx: &Arc<ProjectCtx>,
        w: &Writer,
        action: &Action,
        head: Events,
    ) -> Result<String, String> {
        let Request::Tool { call, .. } = &action.request else {
            unreachable!("only a tool call is approved here")
        };
        // Homebox is asked only for a read held at "Every decision", which takes seconds at most.
        let using = tokio::runtime::Handle::current().block_on(self.using(call));
        let planned = plan(ctx, w, &using, action);
        execute(ctx, w, &using, action, head, planned, true).map_err(|o| match o {
            Outcome::Fenced(why) | Outcome::Refused(why) => why,
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

/// What carrying out a call may use besides the project.
struct Using {
    sandboxes: Option<Arc<Sandboxes>>,
    integrations: Option<Arc<Integrations>>,
    settings: DaemonSettings,
    homebox: crate::integrate::Prefetched,
}

fn carry_out(
    ctx: &Arc<ProjectCtx>,
    using: &Using,
    driver: DriverId,
    call: ToolCall,
) -> Result<String, Outcome> {
    // Fenced under the lock, so the driver cannot end between the check and the append.
    let w = ctx.writer();
    let agent = w.read(|s| fence(s, driver)).map_err(Outcome::Fenced)?;
    let action = Action {
        id: new_id(w.clock(), ActionId::new),
        agent,
        request: Request::Tool { driver, call },
    };
    let ruling = w.read(|s| gate::rule(s, &action));
    let planned = plan(ctx, &w, using, &action);
    // A call the tools refuse is refused whatever the gate says, so the builder is never asked
    // about what would not be done. It reaches the plan, which is what allowed means here.
    if let Err(refusal) = planned {
        let allowed = Ruling::Allow(ruling.classification().clone());
        return refuse(
            &w,
            gate::proposed(&action, &allowed),
            action.id,
            refusal,
            None,
        );
    }
    let head = gate::proposed(&action, &ruling);
    if ruling.is_held() {
        append(&w, head)?;
        let words = gate::held_words(
            action.id,
            &ruling.classification().why,
            gate::on_approve(&action.request),
        );
        return Ok(serde_json::json!({ "held": action.id, "message": words }).to_string());
    }
    execute(ctx, &w, using, &action, head, planned, false)
}

/// The tool call's plan on the state as it is now.
fn plan(
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
fn execute(
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
        match carry_out_process(using.sandboxes.as_deref(), requester, process) {
            Ok(reply) => reply_override = reply,
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
    if let Some(reply) = plan.reply {
        reply_override = Some(reply);
    }
    let mut events = plan.events;
    if let Some(new) = &plan.code_dir {
        match make_code_dir(new) {
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
        write_spec_file(git, *spec, body);
    }
    Ok(reply)
}

/// Starts or stops a process in the agent's sandbox. Blocks: it runs on a blocking thread with the
/// runtime's handle at hand. A start answers with where the process's output goes.
fn carry_out_process(
    sandboxes: Option<&Sandboxes>,
    requester: Requester,
    process: &ProcessPlan,
) -> Result<Option<String>, String> {
    let sandboxes = sandboxes.ok_or("this daemon runs no sandboxes")?;
    let runtime = tokio::runtime::Handle::current();
    match process {
        ProcessPlan::Start { name, command } => {
            let log = runtime.block_on(sandboxes.start_process(requester, name, command))?;
            Ok(Some(
                serde_json::json!({
                    "process": name,
                    "running": true,
                    "output": log.display().to_string(),
                })
                .to_string(),
            ))
        }
        ProcessPlan::Stop { name } => {
            runtime.block_on(sandboxes.stop_process(requester.agent, name))?;
            Ok(None)
        }
    }
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
