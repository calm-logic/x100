//! The handle a driver's owner holds: its output stream and its controls.

use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use x100_core::terminal::TermPos;

use crate::output::{Decision, DriverOutput, PermissionId};
use crate::terminal::Size;
use crate::terminal::pty::Terminal;

/// What the harness reported when it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverInfo {
    /// The harness's own version, as it reported it.
    pub version: String,
    /// Said once at start, such as a version x100 was not tested with.
    pub warnings: Vec<String>,
    /// The first child's process id, which is also its process group id. A terminal driver may
    /// run more than one process; each is given to [`crate::SpawnHook`] as it starts.
    pub pid: u32,
    /// The harness's own id for the conversation, where it has one: Claude Code's session, Codex's
    /// thread.
    pub session: Option<String>,
}

/// A running driver. Dropping the handle and every [`DriverControl`] stops it.
#[derive(Debug)]
pub struct DriverHandle {
    pub(crate) info: DriverInfo,
    pub(crate) control: DriverControl,
    pub(crate) output: mpsc::UnboundedReceiver<DriverOutput>,
}

impl DriverHandle {
    pub fn info(&self) -> &DriverInfo {
        &self.info
    }

    /// The next thing the driver did, or `None` once it has stopped or failed and everything it
    /// emitted has been read.
    pub async fn next(&mut self) -> Option<DriverOutput> {
        self.output.recv().await
    }

    /// What the driver has already emitted, without waiting.
    pub fn try_next(&mut self) -> Option<DriverOutput> {
        self.output.try_recv().ok()
    }

    /// A clonable control, for giving input while another task reads the output.
    pub fn control(&self) -> DriverControl {
        self.control.clone()
    }

    pub async fn turn(&self, input: impl Into<String>) -> Result<(), InputError> {
        self.control.turn(input).await
    }

    pub async fn stop(&self) {
        self.control.stop().await
    }
}

#[derive(Debug, Clone)]
pub struct DriverControl {
    pub(crate) commands: mpsc::UnboundedSender<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("this harness cannot {0}")]
    Unsupported(&'static str),
    #[error("the driver is busy with a turn or a compaction")]
    Busy,
    #[error("the driver is not in a turn")]
    NotInTurn,
    #[error("no permission request {0:?} is waiting")]
    NoSuchRequest(PermissionId),
    #[error("the driver has stopped or failed")]
    Ended,
}

#[derive(Debug)]
pub(crate) enum Command {
    Turn(String, oneshot::Sender<Result<(), InputError>>),
    Answer(
        PermissionId,
        Decision,
        oneshot::Sender<Result<(), InputError>>,
    ),
    Interrupt(oneshot::Sender<Result<(), InputError>>),
    Compact(oneshot::Sender<Result<(), InputError>>),
    Resize(Size, oneshot::Sender<Result<(), InputError>>),
    Screen(oneshot::Sender<(TermPos, String)>),
    /// A terminal started after the driver was ready, and the process in it.
    Attach(Child, Terminal, oneshot::Sender<()>),
    CheckStall(oneshot::Sender<bool>),
    Stop(oneshot::Sender<()>),
}

impl DriverControl {
    async fn ask(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<(), InputError>>) -> Command,
    ) -> Result<(), InputError> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(make(tx))
            .map_err(|_| InputError::Ended)?;
        rx.await.unwrap_or(Err(InputError::Ended))
    }

    /// Starts a turn with this input. Refused while a turn or compaction is running, and on a
    /// harness without further turns once its first turn has run.
    pub async fn turn(&self, input: impl Into<String>) -> Result<(), InputError> {
        let input = input.into();
        self.ask(|tx| Command::Turn(input, tx)).await
    }

    pub async fn answer(&self, id: PermissionId, decision: Decision) -> Result<(), InputError> {
        self.ask(|tx| Command::Answer(id, decision, tx)).await
    }

    /// Ends the running turn early. The turn ends with [`crate::TurnEnd::Interrupted`].
    pub async fn interrupt(&self) -> Result<(), InputError> {
        self.ask(Command::Interrupt).await
    }

    /// Compacts the driver's context between turns. Ends with [`DriverOutput::Compacted`].
    pub async fn compact(&self) -> Result<(), InputError> {
        self.ask(Command::Compact).await
    }

    /// Resizes the driver's terminal. The new size is recorded where the program's output starts
    /// to follow it.
    pub async fn resize(&self, size: Size) -> Result<(), InputError> {
        self.ask(|tx| Command::Resize(size, tx)).await
    }

    /// The text the driver's terminal shows now, or `None` for a driver without one, with the
    /// place of the last chunk it reflects. That chunk and every one before it have been emitted
    /// on the output stream before this returns.
    pub async fn screen(&self) -> Option<(TermPos, String)> {
        let (tx, rx) = oneshot::channel();
        self.commands.send(Command::Screen(tx)).ok()?;
        rx.await.ok()
    }

    pub(crate) async fn attach(&self, child: Child, terminal: Terminal) -> Result<(), InputError> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Attach(child, terminal, tx))
            .map_err(|_| InputError::Ended)?;
        rx.await.map_err(|_| InputError::Ended)
    }

    /// Reads the clock now and fails the driver if it has been silent too long. The supervisor
    /// does this by itself every [`crate::StallPolicy::check_every`]; this is for a caller that
    /// has just moved a fake clock. True if the driver is stalled, and so now failing, or has
    /// already ended.
    pub async fn check_stall(&self) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.commands.send(Command::CheckStall(tx)).is_err() {
            return true;
        }
        rx.await.unwrap_or(true)
    }

    /// Stops the driver and its whole process group, and returns once the child has exited. The
    /// output stream then ends without a failure.
    pub async fn stop(&self) {
        let (tx, rx) = oneshot::channel();
        if self.commands.send(Command::Stop(tx)).is_ok() {
            let _ = rx.await;
        }
    }
}
