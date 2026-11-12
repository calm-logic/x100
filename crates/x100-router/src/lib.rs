//! Finding and allocating drivers, budgets, cool-offs, and the upkeep of warm drivers.
//!
//! Everything here is a pure function of the folded states: the daemon's (harnesses, machines,
//! cool-offs, settings) and the project's (team, ranges, budgets, spend, drivers), plus two things
//! only the running daemon knows: which harnesses it can start ([`Installed`]) and how many live
//! drivers every project together holds on each machine and login ([`Occupancy`]). Which login a
//! harness runs on, and the cap per login, are in the daemon's log. The log is the
//! record, so nothing here is stored; the orchestrator calls these and appends what they decide.
//!
//! - [`choose`]: the agent's warm driver if it is compatible, else the cheapest compatible new
//!   one, else every reason nothing is. Compatible is the table of five rules in [`rules`].
//! - [`upkeep`]: what becomes of a warm driver between turns: kept, compacted or ended.
//! - [`bill`]: who pays for a turn and which budgets the charge reaches.
//! - [`cool_off`] and [`cool_off_over`]: a harness and model cooling off for every project.

mod choose;
mod cool;
mod ledger;
mod rules;
mod upkeep;
mod world;

pub use choose::{Choice, Decision, Ending, Need, Warm, choose};
pub use cool::{cool_off, cool_off_over};
pub use ledger::{Billed, bill};
pub use rules::{blended, needs};
pub use upkeep::{DEFAULT_WARM, Upkeep, upkeep, warm_until};
pub use world::{Installed, Occupancy, Slot, World, slot};
