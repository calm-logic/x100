//! How a started harness is handed to the supervisor: its processes, the channel everything it
//! says arrives on, and where lines and keys for it go.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::Signal;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use x100_core::terminal::TermPos;

use crate::handle::DriverHandle;
use crate::harness::{CommandLine, SpawnHook, StartError};
use crate::process;
use crate::protocol::{HookCall, Protocol};
use crate::supervisor::{self, Launch};
use crate::terminal::pty::Terminal;
use crate::terminal::{Reported, Size};

const STDERR_LINES: usize = 20;

/// What the supervisor hears from a running harness, from whichever source.
#[derive(Debug)]
pub(crate) enum Incoming {
    /// A line of the harness's structured stream: stdout, or a WebSocket frame.
    Line(String),
    /// The structured stream ended; `broken` if it ended with an error.
    Closed {
        broken: bool,
    },
    /// One of the driver's processes exited.
    Exited {
        code: Option<i32>,
    },
    /// Bytes the program wrote to its terminal.
    Output(Vec<u8>),
    /// The terminal has this size from here on.
    Resized(Size),
    Reported(Reported),
    Hook(HookCall),
    /// Nothing holds the terminal open any more.
    TerminalClosed,
    /// The screen asked for with [`crate::DriverControl::screen`], once every chunk before it is emitted.
    Screen(String, oneshot::Sender<(TermPos, String)>),
}

/// The last lines a driver's processes wrote to stderr, for the failure report.
pub(crate) type Tail = Arc<Mutex<VecDeque<String>>>;

/// A started harness, handed to [`launch`]: its processes, how it is heard and how it is told.
pub(crate) struct Wire {
    pub(crate) children: Vec<Child>,
    pub tx: mpsc::UnboundedSender<Incoming>,
    pub(crate) rx: mpsc::UnboundedReceiver<Incoming>,
    /// Where [`Effects::send`] lines go.
    pub lines: Option<mpsc::UnboundedSender<String>>,
    /// Whether closing `lines` asks the harness to exit, so a stop waits for that first.
    pub closing_lines_stops: bool,
    pub terminal: Option<Terminal>,
    pub stderr: Tail,
    /// Tasks that serve the driver, such as its hook endpoint, ended with it.
    pub tasks: Vec<JoinHandle<()>>,
}

impl Wire {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            children: Vec::new(),
            tx,
            rx,
            lines: None,
            closing_lines_stops: false,
            terminal: None,
            stderr: Arc::new(Mutex::new(VecDeque::new())),
            tasks: Vec::new(),
        }
    }

    /// A process of the driver's, the leader of its own process group.
    pub fn add_child(&mut self, child: Child) {
        self.children.push(child);
    }

    /// Keeps the last lines `reader` writes in the stderr tail.
    pub fn tail(&self, reader: impl AsyncRead + Unpin + Send + 'static) {
        let keep = Arc::clone(&self.stderr);
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut tail = keep.lock().expect("stderr tail");
                if tail.len() == STDERR_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });
    }

    /// Kills what was started when the driver cannot be launched after all.
    pub async fn abandon(mut self) {
        for child in &mut self.children {
            if let Some(pid) = child.id() {
                process::signal_group(pid, Signal::SIGKILL);
            }
            let _ = child.wait().await;
        }
        for task in self.tasks {
            task.abort();
        }
    }
}

/// Spawns `line` with piped stdio and launches it: lines on stdout are the harness's stream, and
/// closing stdin asks it to exit. For the headless harnesses and the fake.
pub(crate) async fn launch_piped(
    launch: Launch,
    line: &CommandLine,
    env: &[(String, String)],
    dir: &Path,
    on_spawn: &SpawnHook,
    protocol: Box<dyn Protocol>,
) -> Result<DriverHandle, StartError> {
    let mut child = match process::spawn(line, env, dir) {
        Ok(child) => child,
        Err(e) => {
            remove_dir(launch.cleanup.as_ref()).await;
            return Err(StartError::Spawn(e));
        }
    };
    on_spawn.call(child.id().unwrap_or_default());
    let mut wire = Wire::new();
    let stdout = child.stdout.take().expect("stdout is piped");
    let tx = wire.tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        let broken = loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let _ = tx.send(Incoming::Line(line));
                }
                Ok(None) => break false,
                Err(_) => break true,
            }
        };
        let _ = tx.send(Incoming::Closed { broken });
    });
    wire.lines = Some(spawn_writer(child.stdin.take().expect("stdin is piped")));
    wire.closing_lines_stops = true;
    wire.tail(child.stderr.take().expect("stderr is piped"));
    wire.add_child(child);
    supervisor::launch(launch, protocol, wire).await
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
        // Dropping stdin here closes it, which is how both headless harnesses are asked to exit.
    });
    tx
}

pub(crate) async fn remove_dir(dir: Option<&PathBuf>) {
    if let Some(dir) = dir {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

/// Starting a harness whose process has to be reached before the supervisor runs: how long to
/// wait for it, polled on the real clock.
pub(crate) async fn wait_for<T>(
    limit: Duration,
    mut ready: impl FnMut() -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = ready() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
