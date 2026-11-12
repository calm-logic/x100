//! Everything the store keeps for one project in the daemon's data directory, under the project's
//! id: its event log, transcripts, terminals and files. A project exists before it has code, so
//! none of this is in its code directory; git for that directory is [`crate::Git`], made from the
//! directory the log records.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_core::time::Clock;

use crate::error::{Result, io};
use crate::files::FileStore;
use crate::log::{DaemonLog, ProjectLog};
use crate::terminal::Terminals;
use crate::transcript::Transcripts;

pub const DATABASE: &str = "store.sqlite";

/// A project's event log, transcripts, terminals and files, opened together.
#[derive(Clone)]
pub struct ProjectStore {
    pub log: ProjectLog,
    pub transcripts: Transcripts,
    pub terminals: Terminals,
    pub files: FileStore,
    /// The directory all of this is in.
    pub dir: PathBuf,
}

impl ProjectStore {
    /// Opens the project's store in `dir`, creating it.
    pub fn open(dir: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io(dir))?;
        let db = dir.join(DATABASE);
        let log = ProjectLog::open(&db, clock)?;
        Ok(Self {
            log,
            transcripts: Transcripts::open(&db)?,
            terminals: Terminals::open(&db)?,
            files: FileStore::open(dir.join("files"))?,
            dir: dir.to_owned(),
        })
    }

    /// Where an agent works while the project has no code directory: a directory x100 owns, so
    /// a driver can start before there is anything to check out.
    pub fn scratch(&self) -> PathBuf {
        self.dir.join("scratch")
    }
}

/// Where the daemon keeps project `id`'s store, inside its data directory.
pub fn project_dir(data_dir: &Path, id: impl std::fmt::Display) -> PathBuf {
    data_dir.join("projects").join(id.to_string())
}

/// Opens the daemon's own log, `<dir>/daemon.sqlite`, creating `dir`.
pub fn open_daemon_log(dir: &Path, clock: Arc<dyn Clock>) -> Result<DaemonLog> {
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    DaemonLog::open(&dir.join("daemon.sqlite"), clock)
}

/// The path of a project's database, for tools that look at it directly.
pub fn database_path(store_dir: &Path) -> PathBuf {
    store_dir.join(DATABASE)
}
