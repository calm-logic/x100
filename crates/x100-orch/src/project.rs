//! One project's turn loop: it watches the project's log and decides, for every agent, whether
//! its waiting inbox items go to its warm driver or need a new one. It is the only place that
//! decides, so a driver is never given two turns at once and an agent never has two drivers.
//!
//! Every turn is put to the router first, a further turn on a warm driver included, so a budget
//! that is reached stops the next turn and a warm driver that no longer fits gives way. Between
//! turns the router also says whether a warm driver is kept, compacted or ended. An agent no
//! driver fits waits for one until something that could help happens: a setting, a range, a
//! budget, a cool-off ending, room freeing on the machine.
//!
//! It also takes what the job clock fires, ends a warm driver when its warm period is over, and
//! gives a task up for its agent when its drivers fail three times in a row.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::{FAILURES_BEFORE_GIVING_UP, Failure, RetireReason};
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId, InboxItemId, TaskId};
use x100_core::job::{Job, ProjectJob, ProjectRoutine};
use x100_core::party::Actor;
use x100_core::task::{TaskEvent, TaskFailure, Verdict};
use x100_core::time::Timestamp;
use x100_drivers::terminal::Size;
use x100_router::{Upkeep, Warm};

use crate::Shared;
use crate::ctx::ProjectCtx;
use crate::driver::{self, Cmd, How, Start};
use crate::route::{self, Planned};
use crate::store::blocking;
use crate::{jobs, settle, tools};

pub(crate) enum Ctl {
    Stop {
        driver: DriverId,
        ack: oneshot::Sender<bool>,
    },
    Resize {
        driver: DriverId,
        size: Size,
        ack: oneshot::Sender<bool>,
    },
    /// A job the clock fired, whose `Fired` event is in the log. Acknowledged once the reaction is.
    Fired {
        job: Job<ProjectJob>,
        at: Timestamp,
        ack: oneshot::Sender<()>,
    },
    Shutdown(oneshot::Sender<()>),
}

/// What a driver's task tells the loop.
pub(crate) enum Msg {
    TurnEnded {
        agent: AgentId,
        driver: DriverId,
        /// The router ended the driver with this turn.
        last: bool,
    },
    /// A compaction finished and the driver is warm again.
    Compacted { agent: AgentId, driver: DriverId },
    Exited {
        agent: AgentId,
        driver: DriverId,
        failure: Option<Failure>,
    },
}

struct Live {
    driver: DriverId,
    cmds: mpsc::UnboundedSender<Cmd>,
    /// A turn or a compaction is running or about to start.
    busy: bool,
    /// Every task it has had a turn on.
    carried: BTreeSet<TaskId>,
    /// A task it carried was accepted since it last compacted.
    accepted: bool,
}

impl Live {
    fn warm(&self) -> Warm {
        Warm {
            driver: self.driver,
            carried: self.carried.clone(),
            accepted: self.accepted,
        }
    }
}

struct Loop {
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
    live: HashMap<AgentId, Live>,
    /// Agents no driver fits, until something that could help happens or something new reaches
    /// them.
    no_driver: HashSet<AgentId>,
    /// Agents whose task was given up, after its drivers failed or its wait for a driver ran out,
    /// until something new reaches them. It keeps a driver that fails at once from being started
    /// again without end.
    gave_up: HashSet<AgentId>,
    /// Each agent's driver failures since its last completed turn.
    failures: HashMap<AgentId, Vec<Failure>>,
    /// Where in the log each inbox item arrived, so items go out in the order they came.
    arrived: HashMap<InboxItemId, u64>,
    /// A change to the project's settings or team since warm drivers were last put to the router.
    /// A driver is put to it as its turn ends, in the same append, and otherwise only after such
    /// a change, so what the orchestrator does stays tied to what the log shows.
    tidy: bool,
    /// Tasks an agent accepted whose merge has not finished. The builder's tasks are accepted as
    /// the CEO reports them, so the CEO's driver is not compacted for those.
    reviewed: HashSet<TaskId>,
    msgs: mpsc::UnboundedSender<Msg>,
    /// Every driver's task, the ended ones until they are reaped. A task still runs its last
    /// lines after its driver has ended, so shutdown waits on these, not on the drivers.
    drivers: JoinSet<()>,
    /// Room freed on the machine, or a change in the daemon's log that could help.
    changes: watch::Receiver<u64>,
}

pub(crate) fn spawn(
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
) -> (mpsc::UnboundedSender<Ctl>, JoinHandle<()>) {
    let (_, events) = ctx.store.log.snapshot_and_follow();
    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let (msgs, msgs_rx) = mpsc::unbounded_channel();
    let changes = shared.fleet.watch();
    let lp = Loop {
        shared,
        ctx,
        live: HashMap::new(),
        no_driver: HashSet::new(),
        gave_up: HashSet::new(),
        failures: HashMap::new(),
        arrived: HashMap::new(),
        reviewed: HashSet::new(),
        tidy: false,
        msgs,
        drivers: JoinSet::new(),
        changes,
    };
    (ctl_tx, tokio::spawn(lp.run(events, ctl_rx, msgs_rx)))
}

/// Whether `event` could let an agent waiting for a driver find one.
fn could_help(event: &ProjectEvent) -> bool {
    match event {
        ProjectEvent::SettingChanged(_) => true,
        ProjectEvent::Agent { event, .. } => matches!(
            event,
            AgentEvent::EffortSet(_)
                | AgentEvent::BudgetSet(_)
                | AgentEvent::Promoted { .. }
                | AgentEvent::Moved { .. }
        ),
        ProjectEvent::Task {
            event: TaskEvent::EffortChanged(_),
            ..
        } => true,
        _ => false,
    }
}

impl Loop {
    async fn run(
        mut self,
        mut events: x100_store::Follow<ProjectEvent>,
        mut ctl: mpsc::UnboundedReceiver<Ctl>,
        mut msgs: mpsc::UnboundedReceiver<Msg>,
    ) {
        // Waits whose end a stopped daemon wrote but did not act on.
        self.commit(|s, clock| settle::reconcile(s, clock)).await;
        self.evaluate().await;
        loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(event) = event else { return };
                    match &event.event {
                        ProjectEvent::Agent { agent, event: AgentEvent::Delivered { item, .. } } => {
                            self.arrived.insert(*item, event.seq.0);
                            self.no_driver.remove(agent);
                            self.gave_up.remove(agent);
                        }
                        ProjectEvent::Task { task, event: TaskEvent::Reviewed(Verdict::Accept) }
                            if matches!(event.by, Actor::Agent(_)) =>
                        {
                            self.reviewed.insert(*task);
                        }
                        ProjectEvent::Task { task, event: TaskEvent::Merged { .. } } => {
                            if self.reviewed.remove(task) {
                                self.accepted(*task);
                            }
                        }
                        other if could_help(other) => {
                            self.no_driver.clear();
                            self.tidy = true;
                        }
                        _ => {}
                    }
                }
                Ok(()) = self.changes.changed() => self.no_driver.clear(),
                Some(msg) = msgs.recv() => match msg {
                    Msg::TurnEnded { agent, driver, last } => {
                        self.failures.remove(&agent);
                        if last && self.live.get(&agent).is_some_and(|l| l.driver == driver) {
                            self.live.remove(&agent);
                        }
                        self.idle(agent, driver);
                    }
                    Msg::Compacted { agent, driver } => {
                        if let Some(l) = self.live.get_mut(&agent).filter(|l| l.driver == driver) {
                            l.accepted = false;
                        }
                        self.idle(agent, driver);
                    }
                    Msg::Exited { agent, driver, failure } => {
                        if self.live.get(&agent).is_some_and(|l| l.driver == driver) {
                            self.live.remove(&agent);
                        }
                        if let Some(f) = failure {
                            self.failed(agent, f).await;
                        }
                    }
                },
                Some(_) = self.drivers.join_next(), if !self.drivers.is_empty() => {}
                Some(c) = ctl.recv() => match c {
                    Ctl::Stop { driver, ack } => self.stop(driver, ack),
                    Ctl::Resize { driver, size, ack } => self.resize(driver, size, ack),
                    Ctl::Fired { job, at, ack } => {
                        self.fired(job, at).await;
                        let _ = ack.send(());
                    }
                    Ctl::Shutdown(ack) => {
                        self.shutdown().await;
                        let _ = ack.send(());
                        return;
                    }
                },
            }
            self.tidy().await;
            self.evaluate().await;
        }
    }

    fn idle(&mut self, agent: AgentId, driver: DriverId) {
        if let Some(l) = self.live.get_mut(&agent).filter(|l| l.driver == driver) {
            l.busy = false;
        }
    }

    /// A task was accepted: the driver that carried it is compacted, or retired, before its next
    /// turn.
    fn accepted(&mut self, task: TaskId) {
        for l in self.live.values_mut() {
            if l.carried.contains(&task) {
                l.accepted = true;
            }
        }
    }

    async fn commit(
        &self,
        decide: impl FnOnce(
            &x100_core::fold::ProjectState,
            &dyn x100_core::time::Clock,
        ) -> crate::ctx::Events
        + Send
        + 'static,
    ) {
        let ctx = Arc::clone(&self.ctx);
        if let Err(e) = blocking(move || ctx.commit(|s, c| (decide(s, c), ()))).await {
            eprintln!(
                "x100: project {} could not record a decision: {e}",
                self.ctx.id
            );
        }
    }

    /// Three failures in a row with no turn completed between: the agent stops trying, and its
    /// task is reported failed to its delegator, with each failure.
    async fn failed(&mut self, agent: AgentId, failure: Failure) {
        let failures = self.failures.entry(agent).or_default();
        failures.push(failure);
        if (failures.len() as u32) < FAILURES_BEFORE_GIVING_UP {
            return;
        }
        let failures = self.failures.remove(&agent).unwrap_or_default();
        self.gave_up.insert(agent);
        self.commit(move |s, clock| {
            let Some(task) = s.current_task(agent).map(|t| t.task.id) else {
                return Vec::new();
            };
            let summary = format!("Its drivers failed {} times in a row.", failures.len());
            tools::report_failed(
                s,
                clock,
                task,
                TaskFailure::DriverFailures(failures),
                summary,
            )
        })
        .await;
    }

    async fn fired(&mut self, job: Job<ProjectJob>, at: Timestamp) {
        let done = job.clone();
        self.commit(move |s, clock| jobs::fired(s, clock, &done, at))
            .await;
        match job.does {
            ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver }) => {
                let agent = self
                    .live
                    .iter()
                    .find(|(_, l)| l.driver == driver && !l.busy)
                    .map(|(a, _)| *a);
                if let Some(agent) = agent {
                    self.end(agent, RetireReason::WarmTooLong).await;
                }
            }
            // A wait for a driver that ran out leaves the agent idle with its items: it does not
            // try again until something new reaches it.
            ProjectJob::Routine(ProjectRoutine::DriverWaitDeadline { agent }) => {
                let ctx = Arc::clone(&self.ctx);
                let idle = blocking(move || {
                    ctx.store
                        .log
                        .with_state(|s| s.agent(agent).map(|a| a.state == AgentState::Idle))
                })
                .await;
                if idle == Some(true) {
                    self.gave_up.insert(agent);
                }
            }
            _ => {}
        }
    }

    fn stop(&self, driver: DriverId, ack: oneshot::Sender<bool>) {
        let Some(live) = self.live.values().find(|l| l.driver == driver) else {
            let _ = ack.send(false);
            return;
        };
        let (done, stopped) = oneshot::channel();
        if live.cmds.send(Cmd::Stop(How::Builder, done)).is_err() {
            let _ = ack.send(false);
            return;
        }
        tokio::spawn(async move {
            let _ = ack.send(stopped.await.is_ok());
        });
    }

    /// Told to the driver's task, which owns its terminal. A driver that is not live answers no.
    fn resize(&self, driver: DriverId, size: Size, ack: oneshot::Sender<bool>) {
        match self.live.values().find(|l| l.driver == driver) {
            Some(live) => {
                // If the task is gone, the ack is dropped with the command, which reads as no.
                let _ = live.cmds.send(Cmd::Resize(size, ack));
            }
            None => {
                let _ = ack.send(false);
            }
        }
    }

    /// Ends `agent`'s warm driver and waits until its end is recorded and its process is gone.
    async fn end(&mut self, agent: AgentId, why: RetireReason) {
        let Some(live) = self.live.remove(&agent) else {
            return;
        };
        let (ack, done) = oneshot::channel();
        if live.cmds.send(Cmd::End(why, ack)).is_ok() {
            let _ = done.await;
        }
    }

    async fn shutdown(&mut self) {
        let mut stopped = Vec::new();
        for live in self.live.values() {
            let (done, rx) = oneshot::channel();
            if live.cmds.send(Cmd::Stop(How::Shutdown, done)).is_ok() {
                stopped.push(rx);
            }
        }
        for rx in stopped {
            let _ = rx.await;
        }
        // Drivers that crashed or were just stopped may still be finishing, their process still
        // on record. Once every task has returned, nothing of this project's runs.
        while self.drivers.join_next().await.is_some() {}
    }

    /// After a change to the project's settings or team: every warm driver is kept or ended, as
    /// the router says, one after another. Not while a turn runs, when the orchestrator runs
    /// serially.
    async fn tidy(&mut self) {
        if self.shared.serial && self.live.values().any(|l| l.busy) {
            return;
        }
        if !std::mem::take(&mut self.tidy) {
            return;
        }
        let mut agents: Vec<AgentId> = self.live.keys().copied().collect();
        agents.sort();
        for agent in agents {
            let Some(w) = self.live.get(&agent).filter(|l| !l.busy).map(Live::warm) else {
                continue;
            };
            let (shared, ctx) = (Arc::clone(&self.shared), Arc::clone(&self.ctx));
            if let Upkeep::End(why) = blocking(move || route::upkeep(&shared, &ctx, &w)).await {
                self.end(agent, why).await;
            }
        }
    }

    /// For every agent with items waiting: give them to its warm driver, or start one, as the
    /// router decides. One at a time when the orchestrator runs serially.
    async fn evaluate(&mut self) {
        let serial = self.shared.serial;
        if serial && self.live.values().any(|l| l.busy) {
            return;
        }
        let skip: HashSet<AgentId> = self.no_driver.union(&self.gave_up).copied().collect();
        let ctx = Arc::clone(&self.ctx);
        let mut waiting: Vec<(AgentId, Vec<InboxItemId>)> = blocking(move || {
            ctx.store.log.with_state(|s| {
                s.agents()
                    .filter(|a| a.state != AgentState::Retired && !skip.contains(&a.id))
                    .filter_map(|a| {
                        let items: Vec<_> = a.inbox.waiting().collect();
                        (!items.is_empty()).then_some((a.id, items))
                    })
                    .collect()
            })
        })
        .await;
        let arrived = |i: &InboxItemId| self.arrived.get(i).copied().unwrap_or(0);
        for (_, items) in &mut waiting {
            items.sort_by_key(arrived);
        }
        if serial {
            waiting.sort_by_key(|(_, items)| items.first().map(arrived));
            waiting.truncate(1);
        }
        for (agent, items) in waiting {
            if self.live.get(&agent).is_some_and(|l| l.busy) {
                continue;
            }
            self.turn(agent, items).await;
        }
    }

    async fn turn(&mut self, agent: AgentId, items: Vec<InboxItemId>) {
        let warm = self.live.get(&agent).map(Live::warm);
        let (shared, ctx) = (Arc::clone(&self.shared), Arc::clone(&self.ctx));
        let (planned, task) = blocking(move || route::decide(&shared, &ctx, agent, warm)).await;
        match planned {
            // The turn is decided again once the driver has compacted.
            Ok(Planned::Reuse { compact: true }) => {
                if let Some(live) = self.live.get_mut(&agent) {
                    live.busy = true;
                    let _ = live.cmds.send(Cmd::Compact);
                }
            }
            Ok(Planned::Reuse { compact: false }) => {
                self.commit(move |s, _| route::found(s, agent)).await;
                if let Some(live) = self.live.get_mut(&agent) {
                    live.busy = true;
                    live.carried.extend(task);
                    let carried = live.carried.clone();
                    let _ = live.cmds.send(Cmd::Turn { items, carried });
                }
            }
            Ok(Planned::Start {
                driver,
                choice,
                ending,
            }) => {
                if let Some(e) = ending {
                    self.end(agent, e.why).await;
                }
                self.commit(move |s, _| route::found(s, agent)).await;
                self.start(agent, driver, choice, items, task);
            }
            Err(why) => {
                self.no_driver.insert(agent);
                let deadline = (self.shared.settings)().default_wait_deadline;
                self.commit(move |s, clock| route::no_driver(s, clock, agent, why, deadline))
                    .await;
            }
        }
    }

    fn start(
        &mut self,
        agent: AgentId,
        driver: DriverId,
        choice: x100_router::Choice,
        items: Vec<InboxItemId>,
        task: Option<TaskId>,
    ) {
        let (cmds, rx) = mpsc::unbounded_channel();
        let carried: BTreeSet<TaskId> = task.into_iter().collect();
        self.live.insert(
            agent,
            Live {
                driver,
                cmds,
                busy: true,
                carried: carried.clone(),
                accepted: false,
            },
        );
        let start = Start {
            agent,
            driver,
            choice,
            first: items,
            carried,
        };
        self.drivers.spawn(driver::run(
            Arc::clone(&self.shared),
            Arc::clone(&self.ctx),
            start,
            rx,
            self.msgs.clone(),
        ));
    }
}
