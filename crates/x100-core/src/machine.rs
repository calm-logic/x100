//! What the six state machines share.
//!
//! Each machine is a pure function `next(&State, &Event) -> Result<State, Rejected>` in the module
//! of the record it belongs to: [`crate::agent::next`], [`crate::driver::next`],
//! [`crate::job::next`], [`crate::action::next`], [`crate::task::next`] and [`crate::trial::next`].
//! An event a state does not accept is an error, never a silent no-op, so a log that replays
//! without a rejection is a log the machines allow.

/// The six machines, named in rejections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MachineKind {
    Agent,
    Driver,
    Job,
    Action,
    Task,
    Trial,
}

/// An event that the machine's current state does not accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{machine:?} in state {state} does not accept {event}")]
pub struct Rejected {
    pub machine: MachineKind,
    pub state: &'static str,
    pub event: &'static str,
}
