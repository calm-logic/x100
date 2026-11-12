use std::path::PathBuf;

use x100_core::fold::FoldError;

// The fold errors are boxed so that `Result<_, Error>` stays small for every caller.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The fold refused the event, so nothing was written. The log is unchanged.
    #[error("event refused: {0}")]
    Rejected(Box<FoldError>),
    /// Folding the stored log failed, so the database is damaged. `0` names the event by its
    /// sequence number and the record it is about.
    #[error("the stored log is corrupt: {0}")]
    Corrupt(Box<FoldError>),
    /// A stored row would not parse back into its type.
    #[error("row {row} of {table} cannot be read: {source}")]
    Undecodable {
        table: &'static str,
        row: u64,
        source: serde_json::Error,
    },
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Another process has this store open. Two writers would each fold their own copy of the log.
    #[error("{0} is open in another process")]
    Locked(PathBuf),
    #[error("the database was written by a newer x100 (schema {0})")]
    NewerSchema(i64),
    #[error("no file stored under {0}")]
    NoSuchFile(String),
    #[error("`{0}` is not a content hash")]
    BadHash(String),
    #[error("`{0}` is not a usable name")]
    BadName(String),
    #[error("git {args}: {stderr}")]
    Git { args: String, stderr: String },
    #[error("a worktree already exists at {0}")]
    WorktreeExists(PathBuf),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.into();
    move |source| Error::Io { path, source }
}
