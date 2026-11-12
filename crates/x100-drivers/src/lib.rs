//! Drivers and harnesses.
//!
//! A harness is an agent program (Claude Code, Codex, or the scripted fake) and a driver is one
//! run of it: one child process with one model at one effort level, working for one agent. This
//! crate starts drivers, streams what they do, feeds them input, and notices when they fail. It
//! records nothing: everything a driver emits goes to whoever holds its [`DriverHandle`], which in
//! x100 is the daemon.
//!
//! - [`Harness`] is the trait every harness implements: the models it offers, what it supports,
//!   and [`Harness::start`].
//! - [`DriverHandle`] yields [`DriverOutput`] and, through [`DriverControl`], takes input.
//! - [`claude::ClaudeCode`] runs Claude Code on stream-json, [`codex::Codex`] runs
//!   `codex app-server`, and [`fake::FakeHarness`] plays a script file (see [`fake::script`]).
//! - With the `testing` feature, `testing::FakeProvider` stands in for the providers' APIs.

pub mod claude;
pub mod codex;
mod failure;
pub mod fake;
mod handle;
mod harness;
mod output;
mod process;
mod protocol;
mod supervisor;
#[cfg(feature = "testing")]
pub mod testing;
mod version;

pub use failure::{classify_claude, classify_codex};
pub use handle::{DriverControl, DriverHandle, DriverInfo, InputError};
pub use harness::{
    CommandLine, CommandWrapper, DriverSpec, Harness, Permissions, SpawnHook, StallPolicy,
    StartError, ToolEndpoint,
};
pub use output::{Decision, DriverOutput, PermissionId, PermissionRequest, Spent, TurnEnd};
pub use process::group_is_empty;
