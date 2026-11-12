use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] x100_store::Error),
    #[error("the fake harness script {0} cannot be used: {1}")]
    Script(PathBuf, String),
    /// Another daemon has this data directory.
    #[error("a daemon is already running on {0}")]
    AlreadyRunning(PathBuf),
}
