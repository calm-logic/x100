//! What the supervisor needs from each harness: turning what the harness says (lines on its
//! stream, calls of its hooks, states reported on its terminal) into [`DriverOutput`], and input
//! into lines or keystrokes for it. Process handling is the supervisor's.

use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;
use x100_core::tool::ToolCall;

use crate::handle::InputError;
use crate::output::{Decision, DriverOutput, PermissionId};
use crate::terminal::Reported;

/// Input as the supervisor hands it on, once it has checked the driver can take it.
#[derive(Debug, Clone)]
pub(crate) enum Input {
    Turn(String),
    Answer(PermissionId, Decision),
    Interrupt,
    Compact,
}

/// The harness's handshake finished: what it reported about itself.
#[derive(Debug, Clone)]
pub(crate) struct Ready {
    pub version: String,
    pub warnings: Vec<String>,
    /// The harness's own id for the conversation, such as Codex's thread.
    pub session: Option<String>,
}

/// One call of one of the harness's hooks, which it made to the driver over HTTP.
#[derive(Debug)]
pub(crate) struct HookCall {
    pub event: String,
    pub body: Value,
    /// For a hook whose answer the harness waits on: the JSON to answer with. Dropping it answers
    /// with nothing, which lets the harness go on as if no hook had run.
    pub reply: Option<oneshot::Sender<Value>>,
}

/// Something to type into the driver's terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    Bytes(Vec<u8>),
    /// A wait before the next key, for programs that tell a paste from typing by timing.
    Pause(Duration),
}

/// What handling one line or one input produced.
#[derive(Debug, Default)]
pub(crate) struct Effects {
    pub emit: Vec<DriverOutput>,
    /// Lines for the child's stdin, without the newline.
    pub send: Vec<String>,
    /// Set once, when the handshake ends: ready, or refused with a reason.
    pub started: Option<Result<Ready, String>>,
    /// Calls to the daemon's tools that the supervisor makes on the harness's behalf, by the
    /// harness's own id. Only the fake harness uses this; the real ones call the daemon over MCP.
    pub daemon_calls: Vec<(String, ToolCall)>,
    /// Typed into the driver's terminal, in order.
    pub keys: Vec<Key>,
    /// Asks for [`Protocol::wake`] after this long, on the real clock: for waits that are about a
    /// program's timing, not the agent's time.
    pub wake_after: Option<Duration>,
}

impl Effects {
    pub fn emit(&mut self, out: DriverOutput) {
        self.emit.push(out);
    }

    pub fn send(&mut self, line: impl Into<String>) {
        self.send.push(line.into());
    }

    pub fn send_json(&mut self, value: &serde_json::Value) {
        self.send.push(value.to_string());
    }
}

/// One harness's wire format.
pub(crate) trait Protocol: Send + 'static {
    /// Lines to send as soon as the child is running.
    fn begin(&mut self, fx: &mut Effects);
    /// One line from the child's stdout. A line the harness cannot make sense of is a broken
    /// stream: emit [`DriverOutput::Failed`].
    fn line(&mut self, line: &str, fx: &mut Effects);
    /// Input the supervisor has already checked against the driver's state.
    fn input(&mut self, input: Input, fx: &mut Effects) -> Result<(), InputError>;
    /// The result of a call in [`Effects::daemon_calls`].
    fn daemon_reply(&mut self, _id: String, _reply: Result<String, String>, _fx: &mut Effects) {}
    /// One of the harness's hooks was called.
    fn hook(&mut self, _call: HookCall, _fx: &mut Effects) {}
    /// The program reported its state on its terminal.
    fn reported(&mut self, _report: &Reported, _fx: &mut Effects) {}
    /// The wait asked for with [`Effects::wake_after`] is over.
    fn wake(&mut self, _fx: &mut Effects) {}
}
