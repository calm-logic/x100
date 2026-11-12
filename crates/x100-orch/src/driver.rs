//! One driver's life: record its start, give it turns, record its stream, and end it. A task per
//! driver owns the process handle; the project's loop sends it turns and never touches the handle.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use x100_core::agent::AgentEvent;
use x100_core::agent::AgentState;
use x100_core::driver::{Driver, DriverEvent, Failure, Purpose, StartReason, StopReason};
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId, InboxItemId, MessageId};
use x100_core::inbox::InboxItem;
use x100_core::ledger::{Charge, ChargeSource};
use x100_core::message::{Message, Route};
use x100_core::party::Actor;
use x100_core::transcript::{Entry, Item, TurnEnd as Ended};
use x100_drivers::{
    CommandWrapper, Decision, DriverControl, DriverOutput, DriverSpec, Permissions, SpawnHook,
    Spent, StallPolicy, StartError, TurnEnd,
};
use x100_store::ProjectStore;

use crate::Shared;
use crate::choose::Choice;
use crate::ids::new_id;
use crate::pack;
use crate::project::Msg;
use crate::store::{append, append_all, blocking, read};

pub(crate) enum Cmd {
    /// Give the driver a turn with these inbox items.
    Turn(Vec<InboxItemId>),
    Stop(How, oneshot::Sender<()>),
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
}

/// The turn in progress.
struct Turn {
    text: Vec<String>,
    reply_to: Option<MessageId>,
}

struct Actor_ {
    shared: Arc<Shared>,
    store: ProjectStore,
    agent: AgentId,
    driver: DriverId,
    turn: Option<Turn>,
}

/// Returns whether the driver failed, so the loop does not start another for the same items until
/// something new reaches the agent.
pub(crate) async fn run(
    shared: Arc<Shared>,
    store: ProjectStore,
    start: Start,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    msgs: mpsc::UnboundedSender<Msg>,
) {
    let agent = start.agent;
    let mut actor = Actor_ {
        shared,
        store,
        agent,
        driver: start.driver,
        turn: None,
    };
    let failed = actor.drive(start, &mut cmds, &msgs).await;
    let _ = actor.shared.processes.forget(actor.driver);
    let _ = msgs.send(Msg::Exited { agent, failed });
}

impl Actor_ {
    async fn drive(
        &mut self,
        start: Start,
        cmds: &mut mpsc::UnboundedReceiver<Cmd>,
        msgs: &mpsc::UnboundedSender<Msg>,
    ) -> bool {
        let Start { choice, first, .. } = start;
        let (agent, driver) = (self.agent, self.driver);
        let given = first.clone();
        let (why, pack, previous) = read(&self.store, move |s| {
            let task = given
                .iter()
                .find_map(|i| match s.agent(agent)?.items.get(i)? {
                    InboxItem::Task { task } => Some(*task),
                    _ => None,
                });
            let why = task.map_or(StartReason::Wake, |task| StartReason::NewTask { task });
            let previous = s.agent(agent).and_then(|a| a.drivers.last().copied());
            (why, pack::start_pack(s, agent), previous)
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
            why,
        };
        if let Err(e) = append(
            &self.store,
            Actor::Daemon,
            ProjectEvent::DriverStarted(record),
        )
        .await
        {
            eprintln!("x100: driver {driver} could not be recorded: {e}");
            return true;
        }
        self.transcript(Item::DriverChanged { from: previous })
            .await;

        let dir = match self.worktree().await {
            Ok(dir) => dir,
            Err(detail) => {
                return self
                    .fail(Failure::Error {
                        detail,
                        provider: None,
                    })
                    .await;
            }
        };
        let Some(harness) = self.shared.harnesses.get(&choice.harness).cloned() else {
            let detail = format!("harness {} is not registered", choice.harness);
            return self
                .fail(Failure::Error {
                    detail,
                    provider: None,
                })
                .await;
        };
        let spec = DriverSpec {
            driver,
            model: choice.model,
            effort: choice.effort,
            dir,
            wrap: CommandWrapper::identity(),
            start_pack: pack,
            // The daemon's tools are M6: no endpoint, so a real harness has nothing to call.
            tools: None,
            env: Vec::new(),
            stall: StallPolicy::from_settings(&(self.shared.settings)()),
            permissions: Permissions::AllowAll,
            // Written down before the handshake, since a daemon killed during it would otherwise
            // leave a process nobody recorded. A blocking write, but a short one.
            on_spawn: SpawnHook::new({
                let processes = Arc::clone(&self.shared.processes);
                move |pid| {
                    if let Err(e) = processes.record(driver, pid) {
                        eprintln!("x100: could not write down driver {driver}'s process: {e}");
                    }
                }
            }),
        };
        let mut handle = match harness.start(spec).await {
            Ok(h) => h,
            Err(e) => return self.fail(start_failure(e)).await,
        };
        let control = handle.control();
        if let Err(detail) = self.begin_turn(&control, first).await {
            return self.abort(&control, detail).await;
        }
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Turn(items)) => {
                        if let Err(detail) = self.begin_turn(&control, items).await {
                            return self.abort(&control, detail).await;
                        }
                    }
                    Some(Cmd::Stop(how, ack)) => {
                        control.stop().await;
                        self.stopped(how).await;
                        let _ = ack.send(());
                        return false;
                    }
                    // The loop that owns this driver is gone, so nothing can use it.
                    None => {
                        control.stop().await;
                        self.stopped(How::Shutdown).await;
                        return false;
                    }
                },
                out = handle.next() => match out {
                    Some(DriverOutput::Failed(f)) => return self.fail(f).await,
                    Some(out) => {
                        if let Err(detail) = self.on_output(&control, out, msgs).await {
                            return self.abort(&control, detail).await;
                        }
                    }
                    None => return self.fail(Failure::StreamBroke).await,
                },
            }
        }
    }

    /// Creates the agent's worktree on its own branch the first time a driver needs it.
    async fn worktree(&self) -> Result<std::path::PathBuf, String> {
        let (store, agent) = (self.store.clone(), self.agent);
        blocking(move || {
            let path = store.git.worktree_path(agent);
            if !path.exists() {
                let base = store
                    .git
                    .current_branch(store.git.project_dir())
                    .map_err(|e| format!("could not read the project's branch: {e}"))?;
                store
                    .git
                    .create_worktree(agent, &format!("x100/{agent}"), &base)
                    .map_err(|e| format!("could not create the worktree: {e}"))?;
            }
            Ok(path)
        })
        .await
    }

    async fn event(&self, event: DriverEvent) -> Result<(), String> {
        let event = ProjectEvent::Driver {
            driver: self.driver,
            event,
        };
        append(&self.store, Actor::Daemon, event)
            .await
            .map(drop)
            .map_err(|e| format!("the log refused a driver event: {e}"))
    }

    async fn transcript(&self, item: Item) {
        let entry = Entry {
            at: self.shared.clock.now(),
            driver: self.driver,
            item,
        };
        let (store, agent) = (self.store.clone(), self.agent);
        if let Err(e) = blocking(move || store.transcripts.append(agent, entry)).await {
            eprintln!("x100: transcript of {}: {e}", self.agent);
        }
    }

    /// The driver is healthy but the log would not take what it did: stop it and record why.
    async fn abort(&mut self, control: &DriverControl, detail: String) -> bool {
        control.stop().await;
        self.fail(Failure::Error {
            detail,
            provider: None,
        })
        .await
    }

    /// Records the turn and gives it to the driver.
    async fn begin_turn(
        &mut self,
        control: &DriverControl,
        items: Vec<InboxItemId>,
    ) -> Result<(), String> {
        let (agent, given) = (self.agent, items.clone());
        let (text, reply_to) = read(&self.store, move |s| pack::turn_input(s, agent, &given)).await;
        self.turn = Some(Turn {
            text: Vec::new(),
            reply_to,
        });
        self.transcript(Item::TurnStarted).await;
        self.event(DriverEvent::TurnStarted { given: items })
            .await?;
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
                let reason = "the gate is not built yet, so nothing that needs asking is allowed";
                let _ = control
                    .answer(
                        request.id,
                        Decision::Deny {
                            reason: reason.into(),
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
            DriverOutput::Compacted | DriverOutput::Warning(_) | DriverOutput::Failed(_) => {}
        }
        Ok(())
    }

    async fn charge(&self, spent: Spent) {
        self.transcript(Item::Usage { usage: spent.usage }).await;
        let agent = self.agent;
        let Some(payer) = read(&self.store, move |s| {
            s.project().and_then(|p| p.team.payer(agent))
        })
        .await
        else {
            return;
        };
        let charge = Charge {
            payer,
            source: ChargeSource::Turn {
                driver: self.driver,
                agent: self.agent,
            },
            usage: spent.usage,
            cost: spent.cost,
            basis: spent.basis,
        };
        if let Err(e) = append(&self.store, Actor::Daemon, ProjectEvent::Charged(charge)).await {
            eprintln!("x100: charge for {}: {e}", self.driver);
        }
    }

    /// The turn completed: checkpoint the work, answer the builder if the CEO was asked, and count
    /// the items handled, all in one append. A crash before it gives the items to the next driver,
    /// so a reply can be repeated and is never lost, and none is ever half recorded.
    async fn finish_turn(&mut self, msgs: &mpsc::UnboundedSender<Msg>) -> Result<(), String> {
        let Some(turn) = self.turn.take() else {
            return Ok(());
        };
        let (agent, store) = (self.agent, self.store.clone());
        self.transcript(Item::TurnEnded {
            how: Ended::Completed,
        })
        .await;
        let checkpoint = blocking(move || {
            let dir = store.git.worktree_path(agent);
            store.git.checkpoint(&dir, &agent.to_string())
        })
        .await;
        // One append, so a crash cannot leave a reply without the turn's end, or the end without
        // the handled items it implies: the next driver either repeats the turn or does not.
        let mut events = Vec::new();
        match checkpoint {
            Ok(commit) => {
                events.push((Actor::Daemon, ProjectEvent::Checkpointed { agent, commit }))
            }
            Err(e) => eprintln!("x100: could not checkpoint {agent}: {e}"),
        }
        let reply = turn.text.join("\n\n");
        let is_ceo = read(&self.store, move |s| {
            s.project().is_some_and(|p| p.team.ceo() == agent)
        })
        .await;
        if is_ceo && !reply.trim().is_empty() {
            let message = Message {
                id: new_id(&*self.shared.clock, MessageId::new),
                route: Route::CeoToBuilder,
                body: reply,
                attachments: Vec::new(),
                in_reply_to: turn.reply_to,
            };
            events.push((Actor::Agent(agent), ProjectEvent::MessageSent(message)));
        }
        events.push((
            Actor::Daemon,
            ProjectEvent::Driver {
                driver: self.driver,
                event: DriverEvent::TurnEnded,
            },
        ));
        append_all(&self.store, events)
            .await
            .map_err(|e| format!("the log refused the end of a turn: {e}"))?;
        // Nothing to do and nothing to wait for: the agent rests until something reaches it.
        let resting = read(&self.store, move |s| {
            s.agent(agent).is_some_and(|a| {
                a.state == AgentState::Working
                    && a.open_tasks.is_empty()
                    && a.waits.is_empty()
                    && a.inbox.is_empty()
            })
        })
        .await;
        if resting {
            let event = ProjectEvent::Agent {
                agent,
                event: AgentEvent::WentIdle,
            };
            let _ = append(&self.store, Actor::Daemon, event).await;
        }
        let _ = msgs.send(Msg::TurnEnded { agent });
        Ok(())
    }

    /// The driver failed: the turn it was in is lost and its items wait for the next driver.
    async fn fail(&mut self, failure: Failure) -> bool {
        if self.turn.take().is_some() {
            let detail = format!("{failure:?}");
            self.transcript(Item::TurnEnded {
                how: Ended::Failed { detail },
            })
            .await;
        }
        if let Err(e) = self.event(DriverEvent::Failed(failure)).await {
            eprintln!("x100: {e}");
        }
        true
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
        if let Err(e) = self.event(event).await {
            eprintln!("x100: {e}");
        }
    }
}

fn start_failure(e: StartError) -> Failure {
    match e {
        StartError::Failed { failure, .. } => failure,
        other => Failure::Error {
            detail: other.to_string(),
            provider: None,
        },
    }
}
