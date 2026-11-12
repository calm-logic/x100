//! Opening a SQLite file the way every store here wants it.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::error::{Error, Result, io};

const SCHEMA: i64 = 1;

/// How hard a commit is flushed. Events use `Full`, so a committed event survives a power cut as
/// well as a killed process. Transcripts use `Normal`, which in WAL mode survives a killed process
/// but may lose the last few entries to a power cut, and costs no fsync per streamed token.
#[derive(Clone, Copy)]
pub(crate) enum Durability {
    Full,
    Normal,
}

pub(crate) fn connect(path: &Path, durability: Durability) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    // journal_mode returns a row, so it cannot go through execute_batch's silent path.
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    let sync = match durability {
        Durability::Full => "FULL",
        Durability::Normal => "NORMAL",
    };
    conn.execute_batch(&format!("PRAGMA synchronous = {sync};"))?;
    Ok(conn)
}

/// Creates the tables in `schema` once and refuses a database a newer x100 wrote.
pub(crate) fn migrate(conn: &Connection, schema: &str) -> Result<()> {
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found > SCHEMA {
        return Err(Error::NewerSchema(found));
    }
    conn.execute_batch(schema)?;
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA};"))?;
    Ok(())
}

/// An exclusive claim on a store, released when the process ends however it ends. A killed process
/// cannot leave it behind, which a lock file's mere existence would.
pub(crate) struct Lock {
    _file: File,
}

impl Lock {
    pub(crate) fn take(path: PathBuf) -> Result<Lock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io(&path))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::Locked(path)),
            Err(std::fs::TryLockError::Error(source)) => Err(Error::Io { path, source }),
        }
    }
}
