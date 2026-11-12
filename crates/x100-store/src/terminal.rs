//! Drivers' terminals: the bytes each terminal driver's program wrote and every resize, so a client
//! can replay the screen and follow it live.
//!
//! A terminal is chattier than a transcript, so it has its own table. It shares the transcripts'
//! durability: output is small and frequent, and a crash may lose the last moments but never
//! corrupts the rest. Chunks are numbered from 1 per driver, which is the position a reader resumes
//! from.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, params};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use x100_core::id::DriverId;
use x100_core::terminal::{Chunk, TermPos};

use crate::db::{self, Durability};
use crate::error::{Error, Result};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS terminal (
    driver TEXT NOT NULL,
    pos   INTEGER NOT NULL,
    chunk TEXT NOT NULL,
    PRIMARY KEY (driver, pos)
) STRICT, WITHOUT ROWID;";

/// What a terminal follower receives.
pub type FollowChunks = UnboundedReceiver<(TermPos, Chunk)>;

/// All drivers' terminals in one project. Cloning shares it. Methods block on SQLite.
#[derive(Clone)]
pub struct Terminals {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    conn: Connection,
    next: HashMap<DriverId, u64>,
    followers: HashMap<DriverId, Vec<UnboundedSender<(TermPos, Chunk)>>>,
}

impl Terminals {
    /// Opens the terminal table in the database at `path`, which the event log also uses.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = db::connect(path, Durability::Normal)?;
        db::migrate(&conn, SCHEMA)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                conn,
                next: HashMap::new(),
                followers: HashMap::new(),
            })),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("terminal lock poisoned")
    }

    /// Appends one chunk to a driver's terminal and returns its position.
    pub fn append(&self, driver: DriverId, chunk: Chunk) -> Result<TermPos> {
        let mut g = self.lock();
        let pos = g.next_pos(driver)?;
        let text = serde_json::to_string(&chunk).expect("a chunk serialises");
        let tx = g.conn.transaction()?;
        tx.execute(
            "INSERT INTO terminal (driver, pos, chunk) VALUES (?1, ?2, ?3)",
            params![driver.to_string(), pos as i64, text],
        )?;
        tx.commit()?;
        g.next.insert(driver, pos + 1);
        let pos = TermPos(pos);
        if let Some(fs) = g.followers.get_mut(&driver) {
            fs.retain(|f| f.send((pos, chunk.clone())).is_ok());
        }
        Ok(pos)
    }

    /// The last position of a driver's terminal, or `None` if it has no chunks.
    pub fn last(&self, driver: DriverId) -> Result<Option<TermPos>> {
        let n = self.lock().next_pos(driver)?;
        Ok((n > 1).then_some(TermPos(n - 1)))
    }

    /// Up to `limit` chunks after `after` (from the start for `None`), in order.
    pub fn read(
        &self,
        driver: DriverId,
        after: Option<TermPos>,
        limit: usize,
    ) -> Result<Vec<(TermPos, Chunk)>> {
        read(&self.lock().conn, driver, after, limit)
    }

    /// A receiver of every chunk after `after`: the stored ones first, then new ones as they are
    /// appended. Registered under the same lock as appends, so none is missed or repeated.
    pub fn follow(&self, driver: DriverId, after: Option<TermPos>) -> Result<FollowChunks> {
        let mut g = self.lock();
        let (tx, rx) = unbounded_channel();
        let mut from = after;
        loop {
            let page = read(&g.conn, driver, from, 1000)?;
            let Some(last) = page.last().map(|(p, _)| *p) else {
                break;
            };
            from = Some(last);
            for item in page {
                let _ = tx.send(item);
            }
        }
        g.followers.entry(driver).or_default().push(tx);
        Ok(rx)
    }
}

impl Inner {
    fn next_pos(&mut self, driver: DriverId) -> Result<u64> {
        if let Some(n) = self.next.get(&driver) {
            return Ok(*n);
        }
        let last: Option<i64> = self.conn.query_row(
            "SELECT MAX(pos) FROM terminal WHERE driver = ?1",
            params![driver.to_string()],
            |r| r.get(0),
        )?;
        let n = last.map_or(1, |l| l as u64 + 1);
        self.next.insert(driver, n);
        Ok(n)
    }
}

fn read(
    conn: &Connection,
    driver: DriverId,
    after: Option<TermPos>,
    limit: usize,
) -> Result<Vec<(TermPos, Chunk)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT pos, chunk FROM terminal WHERE driver = ?1 AND pos > ?2 ORDER BY pos LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![
            driver.to_string(),
            after.map_or(0, |p| p.0) as i64,
            limit as i64
        ],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (pos, text) = row?;
        let chunk = serde_json::from_str(&text).map_err(|source| Error::Undecodable {
            table: "terminal",
            row: pos as u64,
            source,
        })?;
        out.push((TermPos(pos as u64), chunk));
    }
    Ok(out)
}
