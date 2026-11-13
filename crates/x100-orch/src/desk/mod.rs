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
//! A call is admitted the moment it arrives, before anything else of the driver is heard, and is
//! counted in flight until it is answered. A driver that is ending is first closed here
//! ([`Desk::close`]): from then on its calls are refused, and the close waits for those in flight,
//! which are carried out to the end. So a call a driver made just before it died takes effect
//! exactly once, and is in the log before its agent's next driver is built a start pack.
//!
//! A call that passes is recorded as an action and put to the gate. One the gate allows is planned,
//! carried out with the git and sandbox work the plan names, and its events appended, all under
//! the project's writer lock. One it holds is answered at once, and carried out the same way, as
//! the agent, if the builder approves it ([`Desk::approved`]).
//! Every credential value in a call is replaced by its reference first, so no message, report,
//! spec, note or file name a driver sends holds one.

mod carry;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::Notify;
use x100_core::action::{Action, Request};
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId, DriverId};
use x100_core::tool::ToolCall;
use x100_drivers::ToolEndpoint;
use x100_drivers::fake::{DaemonTools, Pending};
use x100_integrations::Integrations;
use x100_vault::Redactor;

use crate::ctx::{Events, ProjectCtx, Writer};
use crate::lab::LabCalls;
use crate::sandbox::Sandboxes;
use crate::store::blocking;
use crate::tools::Tool;
use crate::{Settings, Shared};

use carry::{Outcome, Using, carry_out, execute, plan};

/// How many refusals [`Desk::refusals`] keeps, newest last.
const REFUSALS_KEPT: usize = 256;

/// What the desk gave one driver: its token, and the project it works in. Kept for the daemon's
/// life, so a late call from a dead driver is told why it is refused.
struct Pass {
    token: String,
    ctx: Arc<ProjectCtx>,
    /// Set for a lab driver, whose calls are its run's and never the project's.
    lab: Option<Arc<LabCalls>>,
    /// The driver is ending: its calls are refused.
    closed: bool,
    /// Calls admitted and not yet answered.
    in_flight: u32,
    /// The last call it made that reached the log, and its tool.
    last: Option<(ActionId, Tool)>,
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
    /// Told whenever a call in flight is answered, for [`Desk::close`].
    answered: Notify,
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
    /// The orchestrator's shared parts, which a trial's lab runs on. Weak, since they hold the
    /// desk.
    shared: OnceLock<Weak<Shared>>,
}

/// An admitted call, counted in flight while it lives.
struct Ticket {
    desk: Arc<Desk>,
    driver: DriverId,
    ctx: Arc<ProjectCtx>,
    lab: Option<Arc<LabCalls>>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some(pass) = self
            .desk
            .passes
            .lock()
            .expect("desk lock")
            .get_mut(&self.driver)
        {
            pass.in_flight -= 1;
        }
        self.desk.answered.notify_waiters();
    }
}

impl Desk {
    pub fn new(settings: Settings, redactor: Redactor) -> Arc<Self> {
        Arc::new(Self {
            passes: Mutex::new(HashMap::new()),
            answered: Notify::new(),
            refusals: Mutex::new(VecDeque::new()),
            address: OnceLock::new(),
            settings,
            redactor,
            sandboxes: OnceLock::new(),
            integrations: OnceLock::new(),
            shared: OnceLock::new(),
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
            shared: self.shared.get().and_then(Weak::upgrade),
        }
    }

    pub(crate) fn listening_at(&self, base: String) {
        let _ = self.address.set(base);
    }

    pub(crate) fn attach(&self, sandboxes: Arc<Sandboxes>) {
        let _ = self.sandboxes.set(sandboxes);
    }

    pub(crate) fn attach_shared(&self, shared: Weak<Shared>) {
        let _ = self.shared.set(shared);
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
                lab: None,
                closed: false,
                in_flight: 0,
                last: None,
            },
        );
        ToolEndpoint { url, token }
    }

    /// The same for a lab driver, whose calls `calls` answers.
    pub(crate) fn issue_lab(
        &self,
        driver: DriverId,
        ctx: Arc<ProjectCtx>,
        calls: Arc<LabCalls>,
    ) -> ToolEndpoint {
        let endpoint = self.issue(driver, ctx);
        if let Some(pass) = self.passes.lock().expect("desk lock").get_mut(&driver) {
            pass.lab = Some(calls);
        }
        endpoint
    }

    /// Forgets a lab driver once its run is over, so a late call finds nothing to act on.
    pub(crate) fn withdraw(&self, driver: DriverId) {
        self.passes.lock().expect("desk lock").remove(&driver);
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
        let (ctx, lab) = self.project_of(driver, token)?;
        if lab.is_some() {
            return Ok(LabCalls::listing());
        }
        let fenced = blocking(move || {
            ctx.store.log.with_state(|s| {
                let agent = fence(s, driver)?;
                let ceo = s.project().is_some_and(|p| p.team.ceo() == agent);
                Ok(Tool::ALL
                    .into_iter()
                    .filter(|t| ceo || !t.ceo_only())
                    .collect())
            })
        })
        .await;
        fenced.map_err(|why| self.refuse(driver, None, why))
    }

    /// Takes one call from `driver`, admitting it at once, and carries it out on a task of its own,
    /// so a caller that goes away does not cut it short. `Ok` is the result the driver reads;
    /// `Err` says why the call was refused or failed. A call from a driver that may act is an
    /// action in the log either way; one from any other caller changes nothing there.
    pub fn call(self: &Arc<Self>, driver: DriverId, token: &str, call: ToolCall) -> Pending {
        let tool = Tool::of(&call);
        let admitted = self.admit(driver, token, tool);
        let desk = Arc::clone(self);
        let carried = tokio::spawn(async move {
            let ticket = admitted?;
            desk.carry(&ticket, call).await
        });
        Box::pin(async move {
            carried
                .await
                .unwrap_or_else(|e| Err(format!("the call was lost: {e}")))
        })
    }

    async fn carry(&self, ticket: &Ticket, call: ToolCall) -> Result<String, String> {
        let (driver, tool) = (ticket.driver, Tool::of(&call));
        let call = crate::redact::value(&self.redactor, &call)
            .ok_or("the call could not be cleared of credential values, so it is refused")?;
        if let Some(lab) = ticket.lab.clone() {
            return blocking(move || lab.answer(&call)).await;
        }
        let using = self.using(&call).await;
        let ctx = Arc::clone(&ticket.ctx);
        let (action, done) = blocking(move || carry_out(&ctx, &using, driver, call)).await;
        if let Some(action) = action
            && let Some(pass) = self.passes.lock().expect("desk lock").get_mut(&driver)
        {
            pass.last = Some((action, tool));
        }
        match done {
            Err(Outcome::Fenced(why)) => Err(self.refuse(driver, Some(tool), why)),
            Err(Outcome::Refused(why)) => Err(why),
            Ok(reply) => Ok(reply),
        }
    }

    /// Lets in a call from `driver` if the token is the one it was given and it is not ending, and
    /// counts it in flight until the ticket is dropped.
    fn admit(
        self: &Arc<Self>,
        driver: DriverId,
        token: &str,
        tool: Tool,
    ) -> Result<Ticket, String> {
        let mut passes = self.passes.lock().expect("desk lock");
        let Some(pass) = passes.get_mut(&driver).filter(|p| same(&p.token, token)) else {
            drop(passes);
            let why = format!("this token was not given to {driver}: the call is refused");
            return Err(self.refuse(driver, Some(tool), why));
        };
        if pass.closed {
            drop(passes);
            let why = format!("{driver} has stopped acting for its agent: its calls are refused");
            return Err(self.refuse(driver, Some(tool), why));
        }
        pass.in_flight += 1;
        Ok(Ticket {
            desk: Arc::clone(self),
            driver,
            ctx: Arc::clone(&pass.ctx),
            lab: pass.lab.clone(),
        })
    }

    /// For a listing: the project of the driver this token was given to, and its lab run if it
    /// is a lab driver.
    fn project_of(
        &self,
        driver: DriverId,
        token: &str,
    ) -> Result<(Arc<ProjectCtx>, Option<Arc<LabCalls>>), String> {
        let found = self
            .passes
            .lock()
            .expect("desk lock")
            .get(&driver)
            .filter(|p| same(&p.token, token))
            .map(|p| (Arc::clone(&p.ctx), p.lab.clone()));
        found.ok_or_else(|| {
            let why = format!("this token was not given to {driver}: the call is refused");
            self.refuse(driver, None, why)
        })
    }

    /// Refuses `driver`'s calls from now on, and returns once every call it made before is
    /// answered, with the last that reached the log. Called before a driver's end is recorded,
    /// so the calls it made while alive pass the log's fencing and take effect.
    pub(crate) async fn close(&self, driver: DriverId) -> Option<(ActionId, Tool)> {
        loop {
            let answered = self.answered.notified();
            tokio::pin!(answered);
            answered.as_mut().enable();
            {
                let mut passes = self.passes.lock().expect("desk lock");
                let pass = passes.get_mut(&driver)?;
                pass.closed = true;
                if pass.in_flight == 0 {
                    return pass.last;
                }
            }
            answered.await;
        }
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

impl DaemonTools for Desk {
    fn call(
        self: Arc<Self>,
        driver: DriverId,
        endpoint: Option<ToolEndpoint>,
        call: ToolCall,
    ) -> Pending {
        let token = endpoint.map(|e| e.token).unwrap_or_default();
        Desk::call(&self, driver, &token, call)
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
