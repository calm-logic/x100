//! The real clock, scheduled jobs and firing them.
//!
//! A [`Scheduler`] works over one event log, a project's or the daemon's, and holds nothing the log
//! does not: it is rebuilt after a restart by opening the log again. It schedules and cancels
//! jobs, and fires each job that is due by appending the job's `Fired` event. What a fired job
//! does is for the caller, which receives it only after the append. [`step`] decides what is due
//! from a list of jobs and the time, with no I/O, and [`Scheduler::run`] is the thin async loop
//! that sleeps until the next job.

mod clock;
mod error;
mod host;
mod scheduler;
pub mod step;

pub use clock::SystemClock;
pub use error::Error;
pub use host::JobHost;
pub use scheduler::{Fired, Scheduler};
