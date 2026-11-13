//! The params and result of every method.
//!
//! A params type is named for what it asks (`CreateProject`) and has exactly one method, which
//! is how `Call` knows which method a value is for. A result type that holds a record from
//! `x100-core` uses it unchanged, so a client folds and reads the same types the daemon does.

mod daemon;
mod integrations;
mod project;
mod stream;

use serde::{Deserialize, Serialize};

pub use daemon::*;
pub use integrations::*;
pub use project::*;
pub use stream::*;

/// The result of a method that only does something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Done {}
