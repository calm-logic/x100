//! Agents' transcripts: every turn of every driver an agent has had, in x100's own format, so no
//! provider's own transcript is relied on.
//!
//! A transcript is too large and too chatty for the event log, so it has its own table and its own
//! connection. Entries are numbered from 1 per agent, which is the position a reader resumes from.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use x100_core::id::{AgentId, DriverId};
use x100_core::money::Usage;
use x100_core::time::Timestamp;

use crate::db::{self, Durability};
use crate::error::{Error, Result};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transcript (
    agent TEXT NOT NULL,
    pos   INTEGER NOT NULL,
    entry TEXT NOT NULL,
    PRIMARY KEY (agent, pos)
) STRICT, WITHOUT ROWID;";

/// An entry's place in one agent's transcript. Counts from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pos(pub u64);

/// One thing in an agent's transcript, and the driver it came from.
///
/// This shape is also defined, field for field, in `x100-proto`, because clients may not depend on
/// this crate. A test here keeps the two serialisations identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub at: Timestamp,
    pub driver: DriverId,
    pub item: Item,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    TurnStarted,
    /// Text the model wrote.
    Text {
        text: String,
    },
    ToolCall {
        /// Pairs the call with its result.
        call: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        call: String,
        output: String,
        is_error: bool,
    },
    /// What the turn used, as the harness reported it.
    Usage {
        usage: Usage,
    },
    TurnEnded {
        how: TurnEnd,
    },
    /// The first entry of a new driver: the agent's work carries on in the same transcript, with
    /// the change marked. `from` is `None` for the agent's first driver.
    DriverChanged {
        from: Option<DriverId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEnd {
    Completed,
    Failed {
        detail: String,
    },
    /// Stopped while running, by the daemon or the builder.
    Interrupted,
}

/// What a transcript follower receives.
pub type FollowEntries = UnboundedReceiver<(Pos, Entry)>;

/// All agents' transcripts in one project. Cloning shares it. Methods block on SQLite.
#[derive(Clone)]
pub struct Transcripts {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    conn: Connection,
    next: HashMap<AgentId, u64>,
    followers: HashMap<AgentId, Vec<UnboundedSender<(Pos, Entry)>>>,
}

impl Transcripts {
    /// Opens the transcript table in the database at `path`, which the event log also uses.
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
        self.inner.lock().expect("transcript lock poisoned")
    }

    /// Appends one entry to an agent's transcript and returns its position.
    pub fn append(&self, agent: AgentId, entry: Entry) -> Result<Pos> {
        let mut g = self.lock();
        let pos = g.next_pos(agent)?;
        let text = serde_json::to_string(&entry).expect("an entry serialises");
        let tx = g.conn.transaction()?;
        tx.execute(
            "INSERT INTO transcript (agent, pos, entry) VALUES (?1, ?2, ?3)",
            params![agent.to_string(), pos as i64, text],
        )?;
        tx.commit()?;
        g.next.insert(agent, pos + 1);
        let pos = Pos(pos);
        if let Some(fs) = g.followers.get_mut(&agent) {
            fs.retain(|f| f.send((pos, entry.clone())).is_ok());
        }
        Ok(pos)
    }

    /// The last position of an agent's transcript, or `None` if it has no entries.
    pub fn last(&self, agent: AgentId) -> Result<Option<Pos>> {
        let n = self.lock().next_pos(agent)?;
        Ok((n > 1).then_some(Pos(n - 1)))
    }

    /// Up to `limit` entries after `after` (from the start for `None`), in order.
    pub fn read(
        &self,
        agent: AgentId,
        after: Option<Pos>,
        limit: usize,
    ) -> Result<Vec<(Pos, Entry)>> {
        read(&self.lock().conn, agent, after, limit)
    }

    /// A receiver of every entry after `after`: the stored ones first, then new ones as they are
    /// appended. Registered under the same lock as appends, so none is missed or repeated.
    pub fn follow(&self, agent: AgentId, after: Option<Pos>) -> Result<FollowEntries> {
        let mut g = self.lock();
        let (tx, rx) = unbounded_channel();
        let mut from = after;
        loop {
            let page = read(&g.conn, agent, from, 1000)?;
            let Some(last) = page.last().map(|(p, _)| *p) else {
                break;
            };
            from = Some(last);
            for item in page {
                let _ = tx.send(item);
            }
        }
        g.followers.entry(agent).or_default().push(tx);
        Ok(rx)
    }
}

impl Inner {
    fn next_pos(&mut self, agent: AgentId) -> Result<u64> {
        if let Some(n) = self.next.get(&agent) {
            return Ok(*n);
        }
        let last: Option<i64> = self.conn.query_row(
            "SELECT MAX(pos) FROM transcript WHERE agent = ?1",
            params![agent.to_string()],
            |r| r.get(0),
        )?;
        let n = last.map_or(1, |l| l as u64 + 1);
        self.next.insert(agent, n);
        Ok(n)
    }
}

fn read(
    conn: &Connection,
    agent: AgentId,
    after: Option<Pos>,
    limit: usize,
) -> Result<Vec<(Pos, Entry)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT pos, entry FROM transcript WHERE agent = ?1 AND pos > ?2 ORDER BY pos LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![
            agent.to_string(),
            after.map_or(0, |p| p.0) as i64,
            limit as i64
        ],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (pos, text) = row?;
        let entry = serde_json::from_str(&text).map_err(|source| Error::Undecodable {
            table: "transcript",
            row: pos as u64,
            source,
        })?;
        out.push((Pos(pos as u64), entry));
    }
    Ok(out)
}
