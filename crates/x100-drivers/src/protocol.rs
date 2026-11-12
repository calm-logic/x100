//! What the supervisor needs from each harness: turning the child's output lines into
//! [`DriverOutput`] and input into lines for the child. Process handling is the supervisor's.

use x100_core::tool::ToolCall;

use crate::handle::InputError;
use crate::output::{Decision, DriverOutput, PermissionId};

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
}
