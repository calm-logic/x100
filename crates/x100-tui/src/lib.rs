//! `x100`: the terminal interface. A full screen on the daemon's state through `x100-client`, and
//! single commands for scripts. It holds no state of its own beyond what is open.

pub mod ansi;
mod answers;
pub mod app;
pub mod args;
pub mod cli;
mod draw;
pub mod input;
mod keys;
pub mod logfeed;
pub mod msg;
pub mod output;
pub mod page;
mod pages;
pub mod remember;
pub mod run;
pub mod views;
