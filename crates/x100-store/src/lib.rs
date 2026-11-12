//! Everything x100 keeps on disk, and git.
//!
//! * The event logs ([`EventLog`]): one SQLite database per project and one for the daemon. The
//!   folded state is kept in memory and an event is written only if the fold accepts it, so a
//!   stored log always folds.
//! * Agents' transcripts ([`Transcripts`]) in x100's own format.
//! * Attachments and generated files by content hash ([`FileStore`]).
//! * Git for a project directory ([`Git`]): worktrees, hidden-ref checkpoints and merges.
//!
//! The API is synchronous. SQLite and git block, so a tokio daemon calls it from
//! `spawn_blocking`; the channels it returns for following a log are tokio's and are awaited
//! normally. The crate itself starts no runtime and no threads.

mod db;
mod error;
mod files;
pub mod git;
mod log;
mod project;
pub mod transcript;

pub use error::{Error, Result};
pub use files::FileStore;
pub use git::Git;
pub use log::{DaemonLog, EventLog, Folded, Follow, ProjectLog};
pub use project::{DATABASE, ProjectStore, database_path, open_daemon_log};
pub use transcript::Transcripts;
