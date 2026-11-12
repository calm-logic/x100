//! One project's turn loop: it watches the project's log and decides, for every agent, whether
//! its waiting inbox items go to its warm driver or need a new one. It is the only place that
//! decides, so a driver is never given two turns at once and an agent never has two drivers.
//!
//! It also takes what the job clock fires, ends a warm driver when its warm period is over, and
//! gives a task up for its agent when its drivers fail three times in a row.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::{FAILURES_BEFORE_GIVING_UP, Failure, Incompatible};
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId, InboxItemId};
use x100_core::job::{Job, ProjectJob, ProjectRoutine};
use x100_core::party::Actor;
use x100_core::task::TaskFailure;
use x100_core::time::Timestamp;

use crate::Shared;
use crate::ctx::ProjectCtx;
use crate::driver::{self, Cmd, How, Start};
use crate::ids::new_id;
use crate::store::blocking;
use crate::{jobs, settle, tools};

pub(crate) enum Ctl {
    Stop {
        driver: DriverId,
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
    },
    Exited {
        agent: AgentId,
        failure: Option<Failure>,
    },
}

struct Live {
    driver: DriverId,
    cmds: mpsc::UnboundedSender<Cmd>,
    /// A turn is running or about to start.
    busy: bool,
}

struct Loop {
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
    live: HashMap<AgentId, Live>,
    /// Agents that no driver fits, or whose task was given up after its drivers failed, until
    /// something new reaches them. It keeps a driver that fails at once from being started again
    /// without end.
    blocked: HashSet<AgentId>,
    /// Each agent's driver failures since its last completed turn.
    failures: HashMap<AgentId, Vec<Failure>>,
    /// Where in the log each inbox item arrived, so items go out in the order they came.
    arrived: HashMap<InboxItemId, u64>,
    msgs: mpsc::UnboundedSender<Msg>,
    /// Every driver's task, the ended ones until they are reaped. A task still runs its last
    /// lines after its driver has ended, so shutdown waits on these, not on the drivers.
    drivers: JoinSet<()>,
}

pub(crate) fn spawn(
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
) -> (mpsc::UnboundedSender<Ctl>, JoinHandle<()>) {
    let (_, events) = ctx.store.log.snapshot_and_follow();
    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let (msgs, msgs_rx) = mpsc::unbounded_channel();
    let lp = Loop {
        shared,
        ctx,
        live: HashMap::new(),
        blocked: HashSet::new(),
        failures: HashMap::new(),
        arrived: HashMap::new(),
        msgs,
        drivers: JoinSet::new(),
    };
    (ctl_tx, tokio::spawn(lp.run(events, ctl_rx, msgs_rx)))
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
                    if let ProjectEvent::Agent { agent, event: AgentEvent::Delivered { item, .. } } = event.event {
                        self.arrived.insert(item, event.seq.0);
                        self.blocked.remove(&agent);
                    }
                }
                Some(msg) = msgs.recv() => match msg {
                    Msg::TurnEnded { agent } => {
                        self.failures.remove(&agent);
                        if let Some(l) = self.live.get_mut(&agent) {
                            l.busy = false;
                        }
                    }
                    Msg::Exited { agent, failure } => {
                        self.live.remove(&agent);
                        if let Some(f) = failure {
                            self.failed(agent, f).await;
                        }
                    }
                },
                Some(_) = self.drivers.join_next(), if !self.drivers.is_empty() => {}
                Some(c) = ctl.recv() => match c {
                    Ctl::Stop { driver, ack } => self.stop(driver, ack),
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
            self.evaluate().await;
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
        self.blocked.insert(agent);
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
        if let ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver }) = job.does
            && let Some(live) = self.live.values().find(|l| l.driver == driver && !l.busy)
        {
            let (ack, done) = oneshot::channel();
            if live.cmds.send(Cmd::Retire(ack)).is_ok() {
                let _ = done.await;
            }
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

    /// For every agent with items waiting: give them to its warm driver, or start one. One at a
    /// time when the orchestrator runs serially.
    async fn evaluate(&mut self) {
        let serial = self.shared.serial;
        if serial && self.live.values().any(|l| l.busy) {
            return;
        }
        let blocked = self.blocked.clone();
        let ctx = Arc::clone(&self.ctx);
        let mut waiting: Vec<(AgentId, Vec<InboxItemId>)> = blocking(move || {
            ctx.store.log.with_state(|s| {
                s.agents()
                    .filter(|a| a.state != AgentState::Retired && !blocked.contains(&a.id))
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
            match self.live.get_mut(&agent) {
                Some(live) if live.busy => {}
                Some(live) => {
                    live.busy = true;
                    let _ = live.cmds.send(Cmd::Turn(items));
                }
                None => self.start(agent, items).await,
            }
        }
    }

    async fn start(&mut self, agent: AgentId, items: Vec<InboxItemId>) {
        let chooser = Arc::clone(&self.shared.chooser);
        let ctx = Arc::clone(&self.ctx);
        let choice = blocking(move || ctx.store.log.with_state(|s| chooser.choose(s, agent))).await;
        let choice = match choice {
            Ok(c) if self.shared.harnesses.contains_key(&c.harness) => c,
            other => {
                let why = other
                    .err()
                    .unwrap_or_else(|| vec![Incompatible::NothingAllowed]);
                self.blocked.insert(agent);
                self.commit(move |_, _| {
                    vec![(
                        Actor::Daemon,
                        ProjectEvent::Agent {
                            agent,
                            event: AgentEvent::NoDriver(why),
                        },
                    )]
                })
                .await;
                return;
            }
        };
        let driver = new_id(&*self.shared.clock, DriverId::new);
        let (cmds, rx) = mpsc::unbounded_channel();
        self.live.insert(
            agent,
            Live {
                driver,
                cmds,
                busy: true,
            },
        );
        let start = Start {
            agent,
            driver,
            choice,
            first: items,
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
