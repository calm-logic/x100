//! Everything the store keeps for one project, under its `.x100/` directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_core::time::Clock;

use crate::error::{Result, io};
use crate::files::FileStore;
use crate::git::Git;
use crate::log::{DaemonLog, ProjectLog};
use crate::transcript::Transcripts;

pub const DATABASE: &str = "store.sqlite";

/// A project's event log, transcripts, files and git, opened together.
#[derive(Clone)]
pub struct ProjectStore {
    pub log: ProjectLog,
    pub transcripts: Transcripts,
    pub files: FileStore,
    pub git: Git,
}

impl ProjectStore {
    /// Opens `<project>/.x100/`, creating it. Does not touch git: [`Git::prepare`] does that, so
    /// opening a store never changes the project's repository.
    pub fn open(project: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let dir = project.join(".x100");
        std::fs::create_dir_all(&dir).map_err(io(&dir))?;
        let db = dir.join(DATABASE);
        let log = ProjectLog::open(&db, clock)?;
        Ok(Self {
            log,
            transcripts: Transcripts::open(&db)?,
            files: FileStore::open(dir.join("files"))?,
            git: Git::new(project),
        })
    }
}

/// Opens the daemon's own log, `<dir>/daemon.sqlite`, creating `dir`.
pub fn open_daemon_log(dir: &Path, clock: Arc<dyn Clock>) -> Result<DaemonLog> {
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    DaemonLog::open(&dir.join("daemon.sqlite"), clock)
}

/// The path of a project's database, for tools that look at it directly.
pub fn database_path(project: &Path) -> PathBuf {
    project.join(".x100").join(DATABASE)
}
