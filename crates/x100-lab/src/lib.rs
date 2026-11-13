//! Reflection's lab, as rules: how a proposed eval set is checked and completed from the
//! project's log ([`eval`]), how the runs of each arm are scored and whether the trial supports
//! its change ([`rule`]), and what a lab driver is told ([`words`]).
//!
//! Everything here is a pure function of what it is given. The daemon's orchestrator does the
//! work: it writes the change on a trial branch, makes a fresh sandbox from a checkpoint for each
//! run, starts lab drivers with no credentials and no way to write outbound, charges them to the
//! lab's line in the ledger, and merges or deletes the branch as [`rule::rule`] decides.

pub mod eval;
pub mod rule;
pub mod words;

/// Each arm runs at least this many times, because results vary between runs, and at most
/// [`RUNS_MAX`], so a trial's cost has a ceiling the CEO cannot raise.
pub const RUNS_MIN: u8 = 3;
pub const RUNS_MAX: u8 = 10;
/// The most items an eval set may hold.
pub const ITEMS_MAX: usize = 5;
/// How much better the changed arm must be on at least one measure, as a share of the baseline.
pub const MARGIN_PERCENT: u64 = 10;
/// The turns a lab driver gets on one item before it counts as not having reported.
pub const TURNS_PER_ITEM: u32 = 3;
