//! One driver's life: record its start, build its start pack, give it turns, record its stream,
//! and end it. A task per driver owns the process handle; the project's loop sends it turns and
//! never touches the handle.

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use x100_core::driver::{
    Driver, DriverEvent, Failure, Purpose, RetireReason, StartReason, StopReason, View,
};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessModel, ModelOffer};
use x100_core::id::{AgentId, DriverId, InboxItemId, MessageId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::job::JobEvent;
use x100_core::party::{Actor, Party};
use x100_core::task::TaskState;
use x100_core::terminal::Chunk;
use x100_core::transcript::{Entry, Item, TurnEnd as Ended};
use x100_drivers::{Decision, DriverControl, DriverOutput, Spent, StartError, TurnEnd};
use x100_router::Choice;

use crate::Shared;
use crate::branches::{self, Ready};
use crate::ctx::{Events, ProjectCtx};
use crate::pack;
use crate::project::Msg;
use crate::route;
use crate::spec;
use crate::store::blocking;
use crate::turns;

pub(crate) enum Cmd {
    /// Give the driver a turn with these inbox items. `carried` is every task it has had a turn
    /// on, this one's included.
    Turn {
        items: Vec<InboxItemId>,
        carried: BTreeSet<TaskId>,
    },
    Stop(How, oneshot::Sender<()>),
    /// The router retired it between turns.
    End(RetireReason, oneshot::Sender<()>),
    /// A task it carried was accepted and its harness can compact.
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum How {
    /// The builder replaced it.
    Builder,
    /// The daemon is stopping.
    Shutdown,
}

pub(crate) struct Start {
    pub agent: AgentId,
    pub driver: DriverId,
    pub choice: Choice,
    pub first: Vec<InboxItemId>,
    pub carried: BTreeSet<TaskId>,
}

/// The turn in progress.
struct Turn {
    given: Vec<InboxItemId>,
    text: Vec<String>,
    reply_to: Option<MessageId>,
}

struct Actor_ {
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
    agent: AgentId,
    driver: DriverId,
    /// Its record, once it is in the log.
    record: Option<Driver>,
    carried: BTreeSet<TaskId>,
    /// The router ended it with the turn that just finished.
    ended: bool,
    offer: Option<ModelOffer>,
    turn: Option<Turn>,
}

/// Reports how the driver ended, `Some` for a failure, so the loop can count failures in a row.
pub(crate) async fn run(
    shared: Arc<Shared>,
    ctx: Arc<ProjectCtx>,
    start: Start,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    msgs: mpsc::UnboundedSender<Msg>,
) {
    let (agent, driver) = (start.agent, start.driver);
    let mut actor = Actor_ {
        shared,
        ctx,
        agent,
        driver,
        record: None,
        carried: start.carried.clone(),
        ended: false,
        offer: None,
        turn: None,
    };
    let failure = actor.drive(start, &mut cmds, &msgs).await;
    // A file write, so on the blocking pool like the store's, where it holds up no other task.
    let processes = Arc::clone(&actor.shared.processes);
    if let Err(e) = blocking(move || processes.forget(driver)).await {
        eprintln!("x100: could not cross driver {driver}'s process off the list: {e}");
    }
    actor.shared.fleet.release(driver);
    let _ = msgs.send(Msg::Exited {
        agent,
        driver,
        failure,
    });
}

impl Actor_ {
    async fn drive(
        &mut self,
        start: Start,
        cmds: &mut mpsc::UnboundedReceiver<Cmd>,
        msgs: &mpsc::UnboundedSender<Msg>,
    ) -> Option<Failure> {
        let Start { choice, first, .. } = start;
        let (agent, driver) = (self.agent, self.driver);
        let harness = self.shared.harness(choice.harness);
        self.offer = harness
            .as_ref()
            .and_then(|h| h.models().into_iter().find(|m| m.model == choice.model));
        let given = first.clone();
        let (why, previous) = self
            .read(move |s| {
                let task = given
                    .iter()
                    .find_map(|i| match s.agent(agent)?.items.get(i)? {
                        InboxItem::Task { task } => Some(*task),
                        _ => None,
                    });
                let why = task.map_or(StartReason::Wake, |task| StartReason::NewTask { task });
                (why, s.agent(agent).and_then(|a| a.drivers.last().copied()))
            })
            .await;
        let record = Driver {
            id: driver,
            agent,
            harness: choice.harness,
            model: choice.model.clone(),
            effort: choice.effort,
            machine: choice.machine,
            purpose: Purpose::Work,
            view: harness.as_ref().map_or(View::Headless, |h| h.view()),
            why: why.clone(),
        };
        self.record = Some(record.clone());
        if let Err(e) = self
            .append(vec![(Actor::Daemon, ProjectEvent::DriverStarted(record))])
            .await
        {
            eprintln!("x100: driver {driver} could not be recorded: {e}");
            return Some(Failure::Error {
                detail: e,
                provider: None,
            });
        }
        let Some(harness) = harness else {
            let detail = format!("harness {} is not registered", choice.harness);
            return self.fail(error(detail)).await;
        };
        let ready = match self.ready(&first).await {
            Ok(r) => r,
            Err(detail) => return self.fail(error(detail)).await,
        };
        let start_pack = self.start_pack(why, spec::workspace(&harness.kind())).await;
        // The pack is the driver's first entry, so it is on record before the driver can act.
        self.transcript(Item::StartPack {
            text: start_pack.clone(),
        })
        .await;
        self.transcript(Item::DriverChanged { from: previous })
            .await;
        let spec = spec::build(&self.shared, &self.ctx, agent, driver, &choice, start_pack);
        let mut handle = match harness.start(spec).await {
            Ok(h) => h,
            Err(e) => return self.fail(start_failure(e)).await,
        };
        let control = handle.control();
        if let Err(detail) = self.begin_turn(&control, first, ready).await {
            return self.abort(&control, detail).await;
        }
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Turn { items, carried }) => {
                        self.carried = carried;
                        let ready = match self.ready(&items).await {
                            Ok(r) => r,
                            Err(detail) => return self.abort(&control, detail).await,
                        };
                        if let Err(detail) = self.begin_turn(&control, items, ready).await {
                            return self.abort(&control, detail).await;
                        }
                    }
                    // The end is recorded before the process is stopped, so a call it makes while it
                    // dies is refused.
                    Some(Cmd::Stop(how, ack)) => {
                        self.stopped(how).await;
                        control.stop().await;
                        let _ = ack.send(());
                        return None;
                    }
                    Some(Cmd::End(why, ack)) => {
                        self.end(vec![DriverEvent::Retired(why)]).await;
                        control.stop().await;
                        let _ = ack.send(());
                        return None;
                    }
                    Some(Cmd::Compact) => {
                        if let Err(detail) = self.compact(&control).await {
                            return self.abort(&control, detail).await;
                        }
                    }
                    // The loop that owns this driver is gone, so nothing can use it.
                    None => {
                        self.stopped(How::Shutdown).await;
                        control.stop().await;
                        return None;
                    }
                },
                out = handle.next() => match out {
                    Some(DriverOutput::Failed(f)) => return self.fail(f).await,
                    Some(out) => {
                        if let Err(detail) = self.on_output(&control, out, msgs).await {
                            return self.abort(&control, detail).await;
                        }
                        if self.ended {
                            control.stop().await;
                            return None;
                        }
                    }
                    None => return self.fail(Failure::StreamBroke).await,
                },
            }
        }
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&ProjectState) -> T + Send + 'static,
    ) -> T {
        let ctx = Arc::clone(&self.ctx);
        blocking(move || ctx.store.log.with_state(f)).await
    }

    /// Appends through the project's writer, so the waits these events end are closed with them.
    async fn append(&self, events: Events) -> Result<(), String> {
        let ctx = Arc::clone(&self.ctx);
        blocking(move || ctx.writer().append(events).map(drop))
            .await
            .map_err(|e| format!("the log refused a driver's events: {e}"))
    }

    /// The agent's worktree, there and up to date with the work of whoever delegated it a task
    /// among `items`, between turns and never mid-turn.
    async fn ready(&self, items: &[InboxItemId]) -> Result<Ready, String> {
        let (ctx, agent, items) = (Arc::clone(&self.ctx), self.agent, items.to_vec());
        blocking(move || {
            let (home, delegators) = ctx.store.log.with_state(|s| {
                let home = match s.project().and_then(|p| p.team.parent(agent)) {
                    Some(parent) => Party::Agent(parent),
                    None => Party::Builder,
                };
                (home, delegators(s, agent, &items))
            });
            branches::ready(&ctx.store, agent, home, &delegators)
        })
        .await
    }

    async fn start_pack(&self, why: StartReason, workspace: String) -> String {
        let (ctx, agent) = (Arc::clone(&self.ctx), self.agent);
        let window = self.offer.as_ref().map_or(200_000, |o| o.context_window);
        let share = (self.shared.settings)().start_pack_share.value();
        let pack = pack::Pack {
            why,
            workspace,
            window_tokens: window,
            share_percent: share,
        };
        blocking(move || pack::build(&ctx.store, agent, &pack)).await
    }

    async fn event(&self, event: DriverEvent) -> Result<(), String> {
        let event = ProjectEvent::Driver {
            driver: self.driver,
            event,
        };
        self.append(vec![(Actor::Daemon, event)]).await
    }

    async fn transcript(&self, item: Item) {
        let entry = Entry {
            at: self.ctx.clock.now(),
            driver: self.driver,
            item,
        };
        let (ctx, agent) = (Arc::clone(&self.ctx), self.agent);
        if let Err(e) = blocking(move || ctx.store.transcripts.append(agent, entry)).await {
            eprintln!("x100: transcript of {}: {e}", self.agent);
        }
    }

    /// A piece of the driver's terminal, recorded as it arrives so a client follows it live.
    async fn terminal(&self, chunk: Chunk) {
        let (ctx, driver) = (Arc::clone(&self.ctx), self.driver);
        if let Err(e) = blocking(move || ctx.store.terminals.append(driver, chunk)).await {
            eprintln!("x100: terminal of {driver}: {e}");
        }
    }

    /// The driver is healthy but the log would not take what it did: record why and stop it.
    async fn abort(&mut self, control: &DriverControl, detail: String) -> Option<Failure> {
        let failure = self.fail(error(detail)).await;
        control.stop().await;
        failure
    }

    /// Records the turn and gives it to the driver.
    async fn begin_turn(
        &mut self,
        control: &DriverControl,
        items: Vec<InboxItemId>,
        ready: Ready,
    ) -> Result<(), String> {
        let (agent, given, notes) = (self.agent, items.clone(), ready.notes);
        let (text, reply_to) = self
            .read(move |s| pack::turn_input(s, agent, &given, &notes))
            .await;
        self.turn = Some(Turn {
            given: items.clone(),
            text: Vec::new(),
            reply_to,
        });
        self.transcript(Item::TurnStarted).await;
        let (ctx, driver, refreshed) = (Arc::clone(&self.ctx), self.driver, ready.events);
        blocking(move || ctx.commit(|s, _| (turns::started(s, driver, items, refreshed), ())))
            .await
            .map_err(|e| format!("the log refused the start of a turn: {e}"))?;
        // If the driver is already gone, its output ends with the reason and the loop records it.
        let _ = control.turn(text).await;
        Ok(())
    }

    async fn on_output(
        &mut self,
        control: &DriverControl,
        out: DriverOutput,
        msgs: &mpsc::UnboundedSender<Msg>,
    ) -> Result<(), String> {
        match out {
            DriverOutput::Text(text) => {
                if let Some(t) = &mut self.turn {
                    t.text.push(text.clone());
                }
                self.transcript(Item::Text { text }).await;
            }
            DriverOutput::ToolCall { id, name, input } => {
                self.transcript(Item::ToolCall {
                    call: id,
                    name,
                    input,
                })
                .await;
            }
            DriverOutput::ToolResult {
                id,
                output,
                is_error,
            } => {
                self.transcript(Item::ToolResult {
                    call: id,
                    output,
                    is_error,
                })
                .await;
            }
            DriverOutput::PermissionRequest(request) => {
                // The gate is M12. Until then nothing is allowed that the harness would ask about.
                let _ = control
                    .answer(
                        request.id,
                        Decision::Deny {
                            reason: spec::REFUSED.into(),
                        },
                    )
                    .await;
            }
            DriverOutput::Spent(spent) => self.charge(spent).await,
            DriverOutput::Context(fill) => {
                self.event(DriverEvent::ContextReported(fill)).await?;
            }
            DriverOutput::TurnEnded(TurnEnd::Completed) => self.finish_turn(msgs).await?,
            DriverOutput::TurnEnded(TurnEnd::Interrupted) => {
                return Err("a turn was interrupted that nobody asked to interrupt".into());
            }
            DriverOutput::Compacted => {
                self.event(DriverEvent::Compacted).await?;
                let _ = msgs.send(Msg::Compacted {
                    agent: self.agent,
                    driver: self.driver,
                });
            }
            DriverOutput::Terminal(chunk) => self.terminal(chunk).await,
            // Nothing in the log holds a warning yet, such as a login's usage window nearly used,
            // so the daemon's own output is where the builder can see it.
            DriverOutput::Warning(w) => eprintln!("x100: driver {}: {w}", self.driver),
            DriverOutput::Failed(_) => {}
        }
        Ok(())
    }

    async fn charge(&self, spent: Spent) {
        self.transcript(Item::Usage { usage: spent.usage }).await;
        let Some(record) = self.record.clone() else {
            return;
        };
        let ctx = Arc::clone(&self.ctx);
        let charged = blocking(move || {
            ctx.commit(|s, clock| {
                let events =
                    route::charged(s, clock, &record, spent.usage, spent.cost, spent.basis);
                (events, ())
            })
        })
        .await;
        if let Err(e) = charged {
            eprintln!("x100: charge for {}: {e}", self.driver);
        }
    }

    /// A task it carried was accepted: it compacts, and is warm again once it says so.
    async fn compact(&mut self, control: &DriverControl) -> Result<(), String> {
        self.event(DriverEvent::CompactionStarted).await?;
        control
            .compact()
            .await
            .map_err(|e| format!("the driver would not compact: {e}"))
    }

    /// The turn completed: checkpoint the work, answer the builder if the CEO was asked, count the
    /// items handled, and decide what follows, all in one append. A crash before it gives the
    /// items to the next driver, so a reply can be repeated and is never lost, and none is ever
    /// half recorded.
    async fn finish_turn(&mut self, msgs: &mpsc::UnboundedSender<Msg>) -> Result<(), String> {
        let Some(turn) = self.turn.take() else {
            return Ok(());
        };
        self.transcript(Item::TurnEnded {
            how: Ended::Completed,
        })
        .await;
        let (ctx, agent, driver) = (Arc::clone(&self.ctx), self.agent, self.driver);
        let shared = Arc::clone(&self.shared);
        let mut close = turns::Close {
            agent,
            driver,
            record: self.record.clone(),
            carried: self.carried.clone(),
            given: turn.given,
            text: turn.text,
            reply_to: turn.reply_to,
            checkpoint: None,
            limit: (self.shared.settings)().turns_without_progress,
        };
        let ended = blocking(move || {
            let dir = ctx.store.git.worktree_path(agent);
            close.checkpoint = match ctx.store.git.checkpoint(&dir, &agent.to_string()) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("x100: could not checkpoint {agent}: {e}");
                    None
                }
            };
            ctx.commit(|s, clock| turns::close(&shared, s, clock, close))
        })
        .await
        .map_err(|e| format!("the log refused the end of a turn: {e}"))?;
        self.ended = ended;
        let _ = msgs.send(Msg::TurnEnded {
            agent,
            driver,
            last: ended,
        });
        Ok(())
    }

    /// The driver failed: the turn it was in is lost and its items wait for the next driver. A
    /// failure that looks like the provider's cools its harness and model off for every project.
    async fn fail(&mut self, failure: Failure) -> Option<Failure> {
        if let (
            Failure::Error {
                provider: Some(why),
                ..
            },
            Some(r),
        ) = (&failure, &self.record)
        {
            let of = HarnessModel {
                harness: r.harness,
                model: r.model.clone(),
            };
            let (shared, why) = (Arc::clone(&self.shared), why.clone());
            blocking(move || route::cool_off(&shared, of, why)).await;
        }
        if self.turn.take().is_some() {
            let detail = format!("{failure:?}");
            self.transcript(Item::TurnEnded {
                how: Ended::Failed { detail },
            })
            .await;
        }
        self.end(vec![DriverEvent::Failed(failure.clone())]).await;
        Some(failure)
    }

    async fn stopped(&mut self, how: How) {
        let mid_turn = self.turn.take().is_some();
        if mid_turn {
            self.transcript(Item::TurnEnded {
                how: Ended::Interrupted,
            })
            .await;
        }
        let event = match how {
            How::Builder => DriverEvent::Stopped(StopReason::ReplacedByBuilder),
            How::Shutdown if mid_turn => DriverEvent::Failed(Failure::DaemonRestarted),
            How::Shutdown => DriverEvent::Stopped(StopReason::DaemonRestarted),
        };
        self.end(vec![event]).await;
    }

    /// Records the driver's end, and calls off the end of its warm period, which has no driver
    /// left to end.
    async fn end(&self, ending: Vec<DriverEvent>) {
        let (ctx, driver) = (Arc::clone(&self.ctx), self.driver);
        let ended = blocking(move || {
            ctx.commit(|s, _| {
                let mut events: Events = ending
                    .into_iter()
                    .map(|event| (Actor::Daemon, ProjectEvent::Driver { driver, event }))
                    .collect();
                if let Some(job) = turns::warm_period_job(s, driver) {
                    let cancel = JobEvent::Cancelled { by: Actor::Daemon };
                    events.push((Actor::Daemon, ProjectEvent::Job { job, event: cancel }));
                }
                (events, ())
            })
        })
        .await;
        if let Err(e) = ended {
            eprintln!(
                "x100: the end of driver {} is not in the log: {e}",
                self.driver
            );
        }
    }
}

/// Who delegated the tasks among `items` that are in progress with `agent`.
fn delegators(state: &ProjectState, agent: AgentId, items: &[InboxItemId]) -> Vec<Party> {
    let Some(a) = state.agent(agent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        if let Some(InboxItem::Task { task }) = a.items.get(item)
            && let Some(TaskState::InProgress(asg)) = state.task(*task).map(|t| &t.state)
            && asg.assignee == agent
            && !out.contains(&asg.delegator)
        {
            out.push(asg.delegator);
        }
    }
    out
}

fn error(detail: String) -> Failure {
    Failure::Error {
        detail,
        provider: None,
    }
}

fn start_failure(e: StartError) -> Failure {
    match e {
        StartError::Failed { failure, .. } => failure,
        other => error(other.to_string()),
    }
}
