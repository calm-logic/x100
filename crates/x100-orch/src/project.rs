//! One project's turn loop: it watches the project's log and decides, for every agent, whether
//! its waiting inbox items go to its warm driver or need a new one. It is the only place that
//! decides, so a driver is never given two turns at once and an agent never has two drivers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::Incompatible;
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId, InboxItemId};
use x100_core::party::Actor;
use x100_store::ProjectStore;

use crate::Shared;
use crate::driver::{self, Cmd, How, Start};
use crate::ids::new_id;
use crate::store::{append, read};

pub(crate) enum Ctl {
    Stop {
        driver: DriverId,
        ack: oneshot::Sender<bool>,
    },
    Shutdown(oneshot::Sender<()>),
}

/// What a driver's task tells the loop.
pub(crate) enum Msg {
    TurnEnded { agent: AgentId },
    Exited { agent: AgentId, failed: bool },
}

struct Live {
    driver: DriverId,
    cmds: mpsc::UnboundedSender<Cmd>,
    /// A turn is running or about to start.
    busy: bool,
}

struct Loop {
    shared: Arc<Shared>,
    store: ProjectStore,
    live: HashMap<AgentId, Live>,
    /// Agents whose driver failed, or that no driver fits, until something new reaches them. It
    /// keeps a driver that fails at once from being started again without end.
    blocked: HashSet<AgentId>,
    msgs: mpsc::UnboundedSender<Msg>,
}

pub(crate) fn spawn(
    shared: Arc<Shared>,
    store: ProjectStore,
) -> (mpsc::UnboundedSender<Ctl>, JoinHandle<()>) {
    let (_, events) = store.log.snapshot_and_follow();
    let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
    let (msgs, msgs_rx) = mpsc::unbounded_channel();
    let lp = Loop {
        shared,
        store,
        live: HashMap::new(),
        blocked: HashSet::new(),
        msgs,
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
        self.evaluate().await;
        loop {
            tokio::select! {
                event = events.recv() => {
                    let Some(event) = event else { return };
                    if let ProjectEvent::Agent { agent, event: AgentEvent::Delivered { .. } } = event.event {
                        self.blocked.remove(&agent);
                    }
                }
                Some(msg) = msgs.recv() => match msg {
                    Msg::TurnEnded { agent } => {
                        if let Some(l) = self.live.get_mut(&agent) {
                            l.busy = false;
                        }
                    }
                    Msg::Exited { agent, failed } => {
                        self.live.remove(&agent);
                        if failed {
                            self.blocked.insert(agent);
                        }
                    }
                },
                Some(c) = ctl.recv() => match c {
                    Ctl::Stop { driver, ack } => self.stop(driver, ack),
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
    }

    /// For every agent with items waiting: give them to its warm driver, or start one.
    async fn evaluate(&mut self) {
        let blocked = self.blocked.clone();
        let waiting: Vec<(AgentId, Vec<InboxItemId>)> = read(&self.store, move |s| {
            s.agents()
                .filter(|a| a.state != AgentState::Retired && !blocked.contains(&a.id))
                .filter_map(|a| {
                    let items: Vec<_> = a.inbox.waiting().collect();
                    (!items.is_empty()).then_some((a.id, items))
                })
                .collect()
        })
        .await;
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
        let choice = read(&self.store, move |s| chooser.choose(s, agent)).await;
        let choice = match choice {
            Ok(c) if self.shared.harnesses.contains_key(&c.harness) => c,
            other => {
                let why = other
                    .err()
                    .unwrap_or_else(|| vec![Incompatible::NothingAllowed]);
                self.blocked.insert(agent);
                let event = ProjectEvent::Agent {
                    agent,
                    event: AgentEvent::NoDriver(why),
                };
                if let Err(e) = append(&self.store, Actor::Daemon, event).await {
                    eprintln!(
                        "x100: agent {agent} has no driver and the log would not say so: {e}"
                    );
                }
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
        tokio::spawn(driver::run(
            Arc::clone(&self.shared),
            self.store.clone(),
            start,
            rx,
            self.msgs.clone(),
        ));
    }
}
