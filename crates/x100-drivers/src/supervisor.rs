//! Running a driver: owning its processes, passing what the harness says to its protocol and the
//! protocol's answers back, recording its terminal, and noticing failure in each of the four ways:
//! a process exits, the stream breaks, the harness reports an error it cannot retry, or it is
//! silent past the stall timeout.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use x100_core::bytes::Bytes;
use x100_core::driver::Failure;
use x100_core::harness::Support;
use x100_core::terminal::{Chunk, ChunkKind, TermPos};
use x100_core::time::{Clock, Timestamp};
use x100_core::tool::ToolCall;

use crate::handle::{Command, DriverControl, DriverHandle, DriverInfo, InputError};
use crate::harness::{Permissions, StallPolicy, StartError};
use crate::output::{Decision, DriverOutput, PermissionId};
use crate::process;
use crate::protocol::{Effects, Input, Protocol, Ready};
use crate::terminal::pty::Terminal;
use crate::wire::{Incoming, Tail, Wire, remove_dir};

/// Calls one of the daemon's tools for a harness that has no MCP client of its own.
pub(crate) type DaemonCall = Arc<
    dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
>;

/// How long a stopped driver gets to exit after it is asked to, and again after SIGTERM, before
/// its groups are killed. Real time: this is about processes, not about the agent's clock.
const GRACE: Duration = Duration::from_secs(3);
/// How long to keep reading after a process exits, for what it wrote just before.
const DRAIN: Duration = Duration::from_secs(1);

/// What every driver is supervised with.
pub(crate) struct Launch {
    pub support: Support,
    pub stall: StallPolicy,
    pub permissions: Permissions,
    pub clock: Arc<dyn Clock>,
    pub daemon_call: Option<DaemonCall>,
    /// Removed once the driver has ended: its per-driver configuration.
    pub cleanup: Option<PathBuf>,
}

enum StartOutcome {
    Ready(Ready),
    Refused(String),
    Failed(Failure, String),
}

enum End {
    Failed(Failure),
    Refused(String),
    Stopped(Option<oneshot::Sender<()>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Idle,
    Turn,
    Compacting,
}

struct Supervisor {
    protocol: Box<dyn Protocol>,
    pgids: Vec<u32>,
    /// Processes not yet seen to exit.
    alive: usize,
    rx: mpsc::UnboundedReceiver<Incoming>,
    tx: mpsc::UnboundedSender<Incoming>,
    lines: Option<mpsc::UnboundedSender<String>>,
    closing_lines_stops: bool,
    terminal: Option<Terminal>,
    stderr: Tail,
    tasks: Vec<JoinHandle<()>>,
    output: mpsc::UnboundedSender<DriverOutput>,
    started: Option<oneshot::Sender<StartOutcome>>,
    replies_tx: mpsc::UnboundedSender<(String, Result<String, String>)>,
    launch: Launch,
    phase: Phase,
    turns: u32,
    pending: HashSet<PermissionId>,
    tools: HashSet<String>,
    last_activity: Timestamp,
    wake_at: Option<Instant>,
    /// Terminal chunks emitted so far.
    chunks: u64,
}

/// Each child's exit arrives as [`Incoming::Exited`]; its group id is its pid.
fn watch(child: Child, tx: &mpsc::UnboundedSender<Incoming>) -> u32 {
    let pid = child.id().unwrap_or_default();
    let tx = tx.clone();
    let mut child = child;
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code());
        let _ = tx.send(Incoming::Exited { code });
    });
    pid
}

pub(crate) async fn launch(
    launch: Launch,
    protocol: Box<dyn Protocol>,
    wire: Wire,
) -> Result<DriverHandle, StartError> {
    let Wire {
        children,
        tx,
        rx,
        lines,
        closing_lines_stops,
        terminal,
        stderr,
        tasks,
    } = wire;
    let alive = children.len();
    let pgids: Vec<u32> = children.into_iter().map(|c| watch(c, &tx)).collect();
    let pid = pgids.first().copied().unwrap_or_default();
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (started_tx, started_rx) = oneshot::channel();
    let (replies_tx, replies_rx) = mpsc::unbounded_channel();
    let now = launch.clock.now();
    let supervisor = Supervisor {
        protocol,
        pgids,
        alive,
        rx,
        tx,
        lines,
        closing_lines_stops,
        terminal,
        stderr,
        tasks,
        output: output_tx,
        started: Some(started_tx),
        replies_tx,
        launch,
        phase: Phase::Starting,
        turns: 0,
        pending: HashSet::new(),
        tools: HashSet::new(),
        last_activity: now,
        wake_at: None,
        chunks: 0,
    };
    tokio::spawn(supervisor.run(commands_rx, replies_rx));
    let control = DriverControl {
        commands: commands_tx,
    };
    match started_rx.await {
        Ok(StartOutcome::Ready(ready)) => Ok(DriverHandle {
            info: DriverInfo {
                version: ready.version,
                warnings: ready.warnings,
                pid,
                session: ready.session,
            },
            control,
            output: output_rx,
        }),
        Ok(StartOutcome::Refused(reason)) => Err(StartError::Refused(reason)),
        Ok(StartOutcome::Failed(failure, stderr)) => Err(StartError::Failed { failure, stderr }),
        Err(_) => Err(StartError::Failed {
            failure: Failure::StreamBroke,
            stderr: String::new(),
        }),
    }
}

impl Supervisor {
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut replies: mpsc::UnboundedReceiver<(String, Result<String, String>)>,
    ) {
        let mut fx = Effects::default();
        self.protocol.begin(&mut fx);
        let mut end = self.apply(fx);
        let mut ticker = tokio::time::interval(self.launch.stall.check_every);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        while end.is_none() {
            let wake = self.wake_at;
            end = tokio::select! {
                message = self.rx.recv() => match message {
                    Some(message) => self.on_incoming(message).await,
                    None => Some(End::Failed(Failure::StreamBroke)),
                },
                command = commands.recv() => match command {
                    Some(command) => self.on_command(command),
                    // Every handle and control is gone, so nobody can read or stop the driver.
                    None => Some(End::Stopped(None)),
                },
                Some((id, reply)) = replies.recv() => {
                    let mut fx = Effects::default();
                    self.protocol.daemon_reply(id, reply, &mut fx);
                    self.apply(fx)
                },
                _ = ticker.tick() => self.stalled().map(End::Failed),
                _ = tokio::time::sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {
                    self.wake_at = None;
                    let mut fx = Effects::default();
                    self.protocol.wake(&mut fx);
                    self.apply(fx)
                },
            };
        }
        self.finish(end.expect("the loop ends with an end")).await;
    }

    fn chunk(&mut self, what: ChunkKind) {
        self.chunks += 1;
        let at = self.launch.clock.now();
        let _ = self.output.send(DriverOutput::Terminal(Chunk { at, what }));
    }

    /// Everything but the end of a stream or a process.
    fn on_message(&mut self, message: Incoming) -> Option<End> {
        let mut fx = Effects::default();
        match message {
            Incoming::Line(line) => {
                self.last_activity = self.launch.clock.now();
                if line.trim().is_empty() {
                    return None;
                }
                self.protocol.line(&line, &mut fx);
            }
            Incoming::Output(bytes) => {
                // A program that draws is alive; one that has hung stops drawing.
                self.last_activity = self.launch.clock.now();
                self.chunk(ChunkKind::Output {
                    bytes: Bytes(bytes),
                });
                return None;
            }
            Incoming::Resized(size) => {
                self.chunk(ChunkKind::Resize {
                    cols: size.cols,
                    rows: size.rows,
                });
                return None;
            }
            Incoming::Reported(report) => {
                self.last_activity = self.launch.clock.now();
                self.protocol.reported(&report, &mut fx);
            }
            Incoming::Hook(call) => {
                self.last_activity = self.launch.clock.now();
                self.protocol.hook(call, &mut fx);
            }
            Incoming::Screen(text, reply) => {
                let _ = reply.send((TermPos(self.chunks), text));
                return None;
            }
            Incoming::TerminalClosed | Incoming::Closed { .. } | Incoming::Exited { .. } => {
                return None;
            }
        }
        self.apply(fx)
    }

    async fn on_incoming(&mut self, message: Incoming) -> Option<End> {
        match message {
            Incoming::Closed { broken: true } => Some(End::Failed(Failure::StreamBroke)),
            Incoming::Closed { broken: false } => Some(self.stream_ended().await),
            Incoming::Exited { code } => {
                self.alive = self.alive.saturating_sub(1);
                Some(self.drain_after_exit(code).await)
            }
            message => self.on_message(message),
        }
    }

    /// The stream closed. If a process has exited too, that is the failure; if they are all still
    /// running with no way to talk to them, the stream broke.
    async fn stream_ended(&mut self) -> End {
        let deadline = Instant::now() + GRACE;
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, self.rx.recv()).await {
            match message {
                Incoming::Exited { code } => {
                    self.alive = self.alive.saturating_sub(1);
                    return End::Failed(Failure::ProcessExited { code });
                }
                Incoming::Output(_) | Incoming::Resized(_) => {
                    self.on_message(message);
                }
                _ => {}
            }
        }
        End::Failed(Failure::StreamBroke)
    }

    /// A process exited. What it wrote just before may still be unread, and may say why.
    async fn drain_after_exit(&mut self, code: Option<i32>) -> End {
        let deadline = Instant::now() + DRAIN;
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, self.rx.recv()).await {
            match message {
                Incoming::Closed { .. } => break,
                Incoming::Exited { .. } => self.alive = self.alive.saturating_sub(1),
                message => {
                    if let Some(end) = self.on_message(message) {
                        return end;
                    }
                }
            }
        }
        End::Failed(Failure::ProcessExited { code })
    }

    fn stalled(&self) -> Option<Failure> {
        let owes_output = matches!(
            self.phase,
            Phase::Starting | Phase::Turn | Phase::Compacting
        ) && self.pending.is_empty();
        if !owes_output {
            return None;
        }
        let limit = if self.tools.is_empty() {
            self.launch.stall.silent
        } else {
            self.launch.stall.while_tool_runs
        };
        let silent_for = self.launch.clock.now().since(self.last_activity);
        (silent_for > limit).then_some(Failure::Stalled { silent_for })
    }

    fn on_command(&mut self, command: Command) -> Option<End> {
        match command {
            Command::Turn(text, reply) => {
                let checked = if self.phase != Phase::Idle {
                    Err(InputError::Busy)
                } else if self.turns > 0 && !self.launch.support.further_turns {
                    Err(InputError::Unsupported("take a further turn"))
                } else {
                    Ok(())
                };
                self.give(checked, Input::Turn(text), Some(Phase::Turn), reply)
            }
            Command::Answer(id, decision, reply) => {
                let checked = if self.pending.remove(&id) {
                    Ok(())
                } else {
                    Err(InputError::NoSuchRequest(id.clone()))
                };
                self.give(checked, Input::Answer(id, decision), None, reply)
            }
            Command::Interrupt(reply) => {
                let checked = if self.phase == Phase::Turn {
                    Ok(())
                } else {
                    Err(InputError::NotInTurn)
                };
                self.give(checked, Input::Interrupt, None, reply)
            }
            Command::Compact(reply) => {
                let checked = if !self.launch.support.compaction {
                    Err(InputError::Unsupported("compact"))
                } else if self.phase != Phase::Idle {
                    Err(InputError::Busy)
                } else {
                    Ok(())
                };
                self.give(checked, Input::Compact, Some(Phase::Compacting), reply)
            }
            Command::Resize(size, reply) => {
                let _ = reply.send(match &self.terminal {
                    Some(terminal) => {
                        terminal.resize(size);
                        Ok(())
                    }
                    None => Err(InputError::Unsupported("resize a terminal")),
                });
                None
            }
            Command::Screen(reply) => {
                // Without a terminal the reply is dropped, which the caller reads as none.
                if let Some(terminal) = &self.terminal {
                    terminal.screen_now(reply);
                }
                None
            }
            Command::Attach(child, terminal, reply) => {
                self.alive += 1;
                self.pgids.push(watch(child, &self.tx));
                self.terminal = Some(terminal);
                let _ = reply.send(());
                None
            }
            Command::CheckStall(reply) => {
                let end = self.stalled().map(End::Failed);
                let _ = reply.send(end.is_some());
                end
            }
            Command::Stop(reply) => Some(End::Stopped(Some(reply))),
        }
    }

    fn give(
        &mut self,
        checked: Result<(), InputError>,
        input: Input,
        phase: Option<Phase>,
        reply: oneshot::Sender<Result<(), InputError>>,
    ) -> Option<End> {
        if let Err(e) = checked {
            let _ = reply.send(Err(e));
            return None;
        }
        let is_turn = matches!(input, Input::Turn(_));
        let mut fx = Effects::default();
        let result = self.protocol.input(input, &mut fx);
        if result.is_ok() {
            self.last_activity = self.launch.clock.now();
            if let Some(phase) = phase {
                self.phase = phase;
            }
            if is_turn {
                self.turns += 1;
            }
        }
        let _ = reply.send(result);
        self.apply(fx)
    }

    /// Carries out what the protocol asked for, and tracks the driver's state from what it
    /// emitted.
    fn apply(&mut self, fx: Effects) -> Option<End> {
        let mut queue = VecDeque::from([fx]);
        while let Some(fx) = queue.pop_front() {
            if let Some(lines) = &self.lines {
                for line in fx.send {
                    let _ = lines.send(line);
                }
            }
            if !fx.keys.is_empty()
                && let Some(terminal) = &self.terminal
            {
                terminal.type_keys(fx.keys);
            }
            if let Some(wait) = fx.wake_after {
                self.wake_at = Some(Instant::now() + wait);
            }
            for (id, call) in fx.daemon_calls {
                let replies = self.replies_tx.clone();
                match &self.launch.daemon_call {
                    Some(daemon_call) => {
                        let pending = daemon_call(call);
                        tokio::spawn(async move {
                            let _ = replies.send((id, pending.await));
                        });
                    }
                    None => {
                        let _ = replies.send((id, Err("this driver has no daemon tools".into())));
                    }
                }
            }
            match fx.started {
                Some(Ok(ready)) => {
                    self.phase = Phase::Idle;
                    if let Some(started) = self.started.take() {
                        let _ = started.send(StartOutcome::Ready(ready));
                    }
                }
                Some(Err(reason)) => return Some(End::Refused(reason)),
                None => {}
            }
            for out in fx.emit {
                match &out {
                    DriverOutput::Failed(failure) => return Some(End::Failed(failure.clone())),
                    DriverOutput::ToolCall { id, .. } => {
                        self.tools.insert(id.clone());
                    }
                    DriverOutput::ToolResult { id, .. } => {
                        self.tools.remove(id);
                    }
                    DriverOutput::TurnEnded(_) | DriverOutput::Compacted => {
                        self.phase = Phase::Idle;
                        self.tools.clear();
                        self.pending.clear();
                    }
                    DriverOutput::PermissionRequest(request) => {
                        if self.launch.permissions == Permissions::AllowAll {
                            // The harness asked although x100 told it not to: answer for the
                            // daemon, which asked for nothing to be held.
                            let mut more = Effects::default();
                            let input = Input::Answer(request.id.clone(), Decision::Allow);
                            let _ = self.protocol.input(input, &mut more);
                            queue.push_back(more);
                            continue;
                        }
                        self.pending.insert(request.id.clone());
                    }
                    _ => {}
                }
                let _ = self.output.send(out);
            }
        }
        None
    }

    fn signal_all(&self, signal: Signal) {
        for pgid in &self.pgids {
            process::signal_group(*pgid, signal);
        }
    }

    /// Waits up to `limit` for every process to exit, recording the terminal meanwhile.
    async fn wait_exits(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while self.alive > 0 {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(Incoming::Exited { .. })) => self.alive -= 1,
                Ok(Some(message @ (Incoming::Output(_) | Incoming::Resized(_)))) => {
                    self.on_message(message);
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return false,
            }
        }
        true
    }

    /// Records the terminal to its end: the program's last bytes, such as leaving the alternate
    /// screen, come after it is told to stop.
    async fn drain_terminal(&mut self) {
        if self.terminal.take().is_none() {
            return;
        }
        let deadline = Instant::now() + DRAIN;
        while let Ok(Some(message)) = tokio::time::timeout_at(deadline, self.rx.recv()).await {
            match message {
                Incoming::TerminalClosed => break,
                message @ (Incoming::Output(_) | Incoming::Resized(_)) => {
                    self.on_message(message);
                }
                _ => {}
            }
        }
    }

    async fn finish(mut self, end: End) {
        let graceful = matches!(end, End::Stopped(_));
        // Taken before anything is signalled, while what the processes started still has them
        // for a parent: a harness starts background commands in sessions of their own.
        let started = process::descendants(&self.pgids).await;
        let asks_exit = self.closing_lines_stops && self.lines.is_some();
        self.lines = None;
        if !(graceful && asks_exit && self.wait_exits(GRACE).await) {
            self.signal_all(Signal::SIGTERM);
            if !self.wait_exits(GRACE).await {
                self.signal_all(Signal::SIGKILL);
                self.wait_exits(GRACE).await;
            }
        }
        // Whatever the leaders left behind in their groups goes with them.
        self.signal_all(Signal::SIGKILL);
        process::kill_left(&started).await;
        self.drain_terminal().await;
        for task in std::mem::take(&mut self.tasks) {
            task.abort();
        }
        remove_dir(self.launch.cleanup.as_ref()).await;
        let failure = match end {
            End::Failed(failure) => failure,
            End::Refused(reason) => {
                if let Some(started) = self.started.take() {
                    let _ = started.send(StartOutcome::Refused(reason));
                }
                return;
            }
            End::Stopped(reply) => {
                if let Some(reply) = reply {
                    let _ = reply.send(());
                }
                return;
            }
        };
        let tail: Vec<String> = self
            .stderr
            .lock()
            .expect("stderr tail")
            .iter()
            .cloned()
            .collect();
        if let Some(started) = self.started.take() {
            let _ = started.send(StartOutcome::Failed(failure, tail.join("\n")));
            return;
        }
        if !tail.is_empty() {
            let _ = self.output.send(DriverOutput::Warning(format!(
                "last stderr: {}",
                tail.join("\n")
            )));
        }
        let _ = self.output.send(DriverOutput::Failed(failure));
    }
}
