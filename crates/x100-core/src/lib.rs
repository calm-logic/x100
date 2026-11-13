//! The contract every other x100 crate builds on: ids, domain types, the events, the six state
//! machines, the folds of both logs into what each tab shows, and the `Clock` trait with a fake
//! clock for tests. No I/O and no async runtime.
//!
//! The six machines are pure functions `next(&State, &Event) -> Result<State, Rejected>`:
//! [`agent::next`], [`driver::next`], [`job::next`], [`action::next`], [`task::next`] and
//! [`trial::next`].

pub mod action;
pub mod agent;
pub mod bytes;
pub mod credential;
pub mod decision;
pub mod driver;
pub mod effort;
pub mod event;
pub mod file;
pub mod fold;
pub mod git;
pub mod harness;
pub mod id;
pub mod inbox;
pub mod integration;
pub mod job;
pub mod ledger;
pub mod machine;
pub mod memory;
pub mod message;
pub mod money;
pub mod party;
pub mod process;
pub mod project;
pub mod settings;
pub mod spec;
pub mod task;
pub mod team;
pub mod terminal;
#[cfg(feature = "testing")]
pub mod testing;
pub mod time;
pub mod tool;
pub mod transcript;
pub mod trial;
pub mod wait;

#[cfg(test)]
mod verify;

pub use machine::{MachineKind, Rejected};
pub use time::{Clock, FakeClock, Timestamp};
