//! The gate: every action an agent is about to take is labelled with a kind and a risk, and the
//! builder's "Ask me about" level decides which ones wait.
//!
//! The crate is pure. It reads the folded project state and an [`x100_core::action::Action`] and
//! answers with a [`Ruling`]: allow, or hold. For what the rules do not cover it builds a
//! question for a decision model, and the caller, which owns the call, supplies what came of it.
//! It never lets an action through because an answer is missing.
//!
//! - [`rule`]: the table, as data. Read `ALL` top to bottom for the policy.
//! - [`assess`] and [`judge`]: rules, then the classifier, then the level.
//! - [`decision`]: one trait and an adapter per decision model, which build the request and read
//!   the answer.
//! - [`Shown`]: what a held action shows the builder.
//! - [`resume`]: what happens on Approve and on Deny, for each kind of request.
//! - [`AllowedOnce`]: the record an Approve of a command or egress request leaves.

mod classify;
mod command;
pub mod decision;
mod egress;
mod gate;
mod once;
mod resume;
pub mod rule;
mod shown;
mod tools;

pub use classify::{Asked, Classifier, Labels, Pending, Question};
pub use command::watched_programs;
pub use egress::{Settled, settled};
pub use gate::{Ruling, Verdict, assess, holds, judge};
pub use once::{AllowedOnce, Grant, Permit};
pub use resume::{AtOnce, Effect, Resume, resume, tell_agent};
pub use shown::{Shown, TaskLine, Who};
