//! Running a driver: owning its child process, passing lines between it and its protocol, and
//! noticing failure in each of the four ways: the process exits, the stream breaks, the harness
//! reports an error it cannot retry, or it is silent past the stall timeout.

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{MissedTickBehavior, timeout};
use x100_core::driver::Failure;
use x100_core::harness::Support;
use x100_core::time::{Clock, Timestamp};
use x100_core::tool::ToolCall;

use crate::handle::{Command, DriverControl, DriverHandle, DriverInfo, InputError};
use crate::harness::{CommandLine, Permissions, SpawnHook, StallPolicy, StartError};
use crate::output::{Decision, DriverOutput, PermissionId};
use crate::process;
use crate::protocol::{Effects, Input, Protocol, Ready};

/// Calls one of the daemon's tools for a harness that has no MCP client of its own.
pub(crate) type DaemonCall = Arc<
    dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
>;

/// How long a stopped child gets to exit after its stdin closes, and again after SIGTERM, before
/// the group is killed. Real time: this is about processes, not about the agent's clock.
const GRACE: Duration = Duration::from_secs(3);
/// How long to keep reading stdout after the child exits, for lines it wrote just before.
const DRAIN: Duration = Duration::from_secs(1);
const STDERR_LINES: usize = 20;

pub(crate) struct Launch {
    pub line: CommandLine,
    pub env: Vec<(String, String)>,
    pub dir: PathBuf,
    pub support: Support,
    pub stall: StallPolicy,
    pub permissions: Permissions,
    pub clock: Arc<dyn Clock>,
    pub daemon_call: Option<DaemonCall>,
    /// Removed once the driver has ended: its per-driver configuration.
    pub cleanup: Option<PathBuf>,
    pub on_spawn: SpawnHook,
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
    child: Child,
    pgid: u32,
    stdout: Lines<BufReader<ChildStdout>>,
    stdin: Option<mpsc::UnboundedSender<String>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    output: mpsc::UnboundedSender<DriverOutput>,
    started: Option<oneshot::Sender<StartOutcome>>,
    replies_tx: mpsc::UnboundedSender<(String, Result<String, String>)>,
    launch: Launch,
    phase: Phase,
    turns: u32,
    pending: HashSet<PermissionId>,
    tools: HashSet<String>,
    last_activity: Timestamp,
}

pub(crate) async fn launch(
    launch: Launch,
    protocol: Box<dyn Protocol>,
) -> Result<DriverHandle, StartError> {
    let mut child = match process::spawn(&launch.line, &launch.env, &launch.dir) {
        Ok(child) => child,
        Err(e) => {
            remove_dir(launch.cleanup.as_ref()).await;
            return Err(StartError::Spawn(e));
        }
    };
    let pid = child.id().unwrap_or_default();
    launch.on_spawn.call(pid);
    let stdout = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
    let stdin = spawn_writer(child.stdin.take().expect("stdin is piped"));
    let stderr = spawn_stderr(child.stderr.take().expect("stderr is piped"));
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (started_tx, started_rx) = oneshot::channel();
    let (replies_tx, replies_rx) = mpsc::unbounded_channel();
    let now = launch.clock.now();
    let supervisor = Supervisor {
        protocol,
        child,
        pgid: pid,
        stdout,
        stdin: Some(stdin),
        stderr,
        output: output_tx,
        started: Some(started_tx),
        replies_tx,
        launch,
        phase: Phase::Starting,
        turns: 0,
        pending: HashSet::new(),
        tools: HashSet::new(),
        last_activity: now,
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

fn spawn_writer(mut stdin: tokio::process::ChildStdin) -> mpsc::UnboundedSender<String> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(mut line) = rx.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
        // Dropping stdin here closes it, which is how both real harnesses are asked to exit.
    });
    tx
}

fn spawn_stderr(stderr: tokio::process::ChildStderr) -> Arc<Mutex<VecDeque<String>>> {
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let keep = Arc::clone(&tail);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut tail = keep.lock().expect("stderr tail");
            if tail.len() == STDERR_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });
    tail
}

async fn remove_dir(dir: Option<&PathBuf>) {
    if let Some(dir) = dir {
        let _ = tokio::fs::remove_dir_all(dir).await;
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
            end = tokio::select! {
                line = self.stdout.next_line() => match line {
                    Ok(Some(line)) => self.on_line(&line),
                    Ok(None) => Some(self.stream_ended().await),
                    Err(_) => Some(End::Failed(Failure::StreamBroke)),
                },
                status = self.child.wait() => {
                    let code = status.ok().and_then(|s| s.code());
                    Some(self.drain_after_exit(code).await)
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
            };
        }
        self.finish(end.expect("the loop ends with an end")).await;
    }

    fn on_line(&mut self, line: &str) -> Option<End> {
        self.last_activity = self.launch.clock.now();
        if line.trim().is_empty() {
            return None;
        }
        let mut fx = Effects::default();
        self.protocol.line(line, &mut fx);
        self.apply(fx)
    }

    /// Stdout closed. If the child has exited too, that is the failure; if it is still running
    /// with no way to talk to it, the stream broke.
    async fn stream_ended(&mut self) -> End {
        match timeout(GRACE, self.child.wait()).await {
            Ok(status) => End::Failed(Failure::ProcessExited {
                code: status.ok().and_then(|s| s.code()),
            }),
            Err(_) => End::Failed(Failure::StreamBroke),
        }
    }

    /// The child exited. Lines it wrote just before may still be unread, and may say why.
    async fn drain_after_exit(&mut self, code: Option<i32>) -> End {
        while let Ok(Ok(Some(line))) = timeout(DRAIN, self.stdout.next_line()).await {
            if let Some(end) = self.on_line(&line) {
                return end;
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
            if let Some(stdin) = &self.stdin {
                for line in fx.send {
                    let _ = stdin.send(line);
                }
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

    async fn finish(mut self, end: End) {
        let graceful = matches!(end, End::Stopped(_));
        // Closing stdin asks both real harnesses to exit; the fake exits too.
        self.stdin = None;
        if !(graceful && timeout(GRACE, self.child.wait()).await.is_ok()) {
            process::signal_group(self.pgid, Signal::SIGTERM);
            if timeout(GRACE, self.child.wait()).await.is_err() {
                process::signal_group(self.pgid, Signal::SIGKILL);
                let _ = self.child.wait().await;
            }
        }
        // Whatever the leader left behind in its group goes with it.
        process::signal_group(self.pgid, Signal::SIGKILL);
        let failure = match end {
            End::Failed(failure) => Some(failure),
            End::Refused(reason) => {
                if let Some(started) = self.started.take() {
                    let _ = started.send(StartOutcome::Refused(reason));
                }
                None
            }
            End::Stopped(reply) => {
                remove_dir(self.launch.cleanup.as_ref()).await;
                if let Some(reply) = reply {
                    let _ = reply.send(());
                }
                return;
            }
        };
        remove_dir(self.launch.cleanup.as_ref()).await;
        if let Some(failure) = failure {
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
}
