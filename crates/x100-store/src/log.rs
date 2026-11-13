//! An event log in SQLite with its folded state kept in memory.
//!
//! The state is the only judge of what may be appended: an event is applied to it first, and
//! written only if the fold accepts it, so a stored log always folds. The state and the table
//! change together under one lock, and a subscriber is registered under the same lock, so a
//! snapshot and the stream after it have no gap and no repeat.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent, Seq};
use x100_core::fold::{DaemonState, FoldError, ProjectState};
use x100_core::party::Actor;
use x100_core::time::Clock;

use crate::db::{self, Durability, Lock};
use crate::error::{Error, Result};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events (
    seq   INTEGER PRIMARY KEY,
    at    INTEGER NOT NULL,
    by    TEXT NOT NULL,
    event TEXT NOT NULL
) STRICT;";

/// A state that an event log folds into.
pub trait Folded: Default + Clone + Send + 'static {
    type Event: Clone + Serialize + DeserializeOwned + Send + 'static;

    fn apply(&mut self, e: &Envelope<Self::Event>) -> Result<(), FoldError>;
    fn last(&self) -> Option<Seq>;
}

impl Folded for ProjectState {
    type Event = ProjectEvent;

    fn apply(&mut self, e: &Envelope<ProjectEvent>) -> Result<(), FoldError> {
        ProjectState::apply(self, e).map(drop)
    }

    fn last(&self) -> Option<Seq> {
        self.seq()
    }
}

impl Folded for DaemonState {
    type Event = DaemonEvent;

    fn apply(&mut self, e: &Envelope<DaemonEvent>) -> Result<(), FoldError> {
        DaemonState::apply(self, e).map(drop)
    }

    fn last(&self) -> Option<Seq> {
        self.seq()
    }
}

pub type ProjectLog = EventLog<ProjectState>;
pub type DaemonLog = EventLog<DaemonState>;

/// What a follower receives: every event after the point it asked for, in order, once.
pub type Follow<E> = UnboundedReceiver<Envelope<E>>;

/// One log. Cloning shares it. All methods block on SQLite, so an async caller runs them on a
/// blocking thread (`spawn_blocking`), never on an executor thread.
pub struct EventLog<S: Folded> {
    inner: Arc<Inner<S>>,
}

impl<S: Folded> Clone for EventLog<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct Inner<S: Folded> {
    clock: Arc<dyn Clock>,
    shared: Mutex<Shared<S>>,
    _lock: Lock,
}

struct Shared<S: Folded> {
    conn: Connection,
    state: S,
    followers: Vec<UnboundedSender<Envelope<S::Event>>>,
}

impl<S: Folded> EventLog<S> {
    /// Opens (creating if needed) the database at `path` and folds its log. An event the fold
    /// rejects is [`Error::Corrupt`], naming its sequence number.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let lock = Lock::take(path.with_extension("lock"))?;
        let conn = db::connect(path, Durability::Full)?;
        db::migrate(&conn, SCHEMA)?;
        let state = fold_stored::<S>(&conn)?;
        Ok(Self {
            inner: Arc::new(Inner {
                clock,
                shared: Mutex::new(Shared {
                    conn,
                    state,
                    followers: Vec::new(),
                }),
                _lock: lock,
            }),
        })
    }

    fn shared(&self) -> MutexGuard<'_, Shared<S>> {
        // A panic while holding the lock leaves state we cannot vouch for; nothing to recover.
        self.inner.shared.lock().expect("event log lock poisoned")
    }

    /// Appends one event and returns it with its sequence number and time.
    ///
    /// The event is refused with [`Error::Rejected`] if the fold does not accept it.
    pub fn append(&self, by: Actor, event: S::Event) -> Result<Envelope<S::Event>> {
        let mut done = self.append_all(vec![(by, event)])?;
        Ok(done.pop().expect("one event in, one out"))
    }

    /// Appends events as one unit and returns them with their sequence numbers and times.
    ///
    /// They are folded in order and written in one transaction, so after a crash all are in the
    /// log or none is, and a follower sees them in order with no other event between. If the fold
    /// refuses any of them the result is [`Error::Rejected`], nothing is written and the state is
    /// as it was.
    pub fn append_all(&self, events: Vec<(Actor, S::Event)>) -> Result<Vec<Envelope<S::Event>>> {
        let mut sh = self.shared();
        let first = sh.state.last().map_or(1, |s| s.0 + 1);
        let envelopes: Vec<_> = events
            .into_iter()
            .zip(first..)
            .map(|((by, event), seq)| Envelope {
                seq: Seq(seq),
                at: self.inner.clock.now(),
                by,
                event,
            })
            .collect();
        // The fold may leave the state part-way through an event when it refuses, and earlier
        // events of the batch are already applied. Either way the state is rebuilt from the table,
        // which holds none of this batch. Failures are rare, and cloning the state for every
        // append is not.
        for envelope in &envelopes {
            if let Err(e) = sh.state.apply(envelope) {
                sh.state = fold_stored(&sh.conn)?;
                return Err(Error::Rejected(Box::new(e)));
            }
        }
        if let Err(e) = insert(&mut sh.conn, &envelopes) {
            sh.state = fold_stored(&sh.conn)?;
            return Err(e);
        }
        // Still under the lock, so no other append's events can fall between these.
        for envelope in &envelopes {
            sh.followers.retain(|f| f.send(envelope.clone()).is_ok());
        }
        Ok(envelopes)
    }

    /// The last sequence number, or `None` for an empty log.
    pub fn last(&self) -> Option<Seq> {
        self.shared().state.last()
    }

    /// A copy of the folded state.
    pub fn state(&self) -> S {
        self.shared().state.clone()
    }

    /// Reads the state without copying it.
    pub fn with_state<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        f(&self.shared().state)
    }

    /// Up to `limit` events after `after` (from the start for `None`), oldest first.
    pub fn read_after(&self, after: Option<Seq>, limit: usize) -> Result<Vec<Envelope<S::Event>>> {
        read::<S::Event>(&self.shared().conn, Dir::After(after), limit)
    }

    /// Up to `limit` events before `before` (from the end for `None`), newest first, for a
    /// view that opens at the latest and reads back.
    pub fn read_before(
        &self,
        before: Option<Seq>,
        limit: usize,
    ) -> Result<Vec<Envelope<S::Event>>> {
        read::<S::Event>(&self.shared().conn, Dir::Before(before), limit)
    }

    /// Up to `limit` events before `before` (from the end for `None`), newest first, whose stored
    /// text holds every one of `terms`, ignoring ASCII case. The text is the event as JSON, so a
    /// term matches inside any string the event carries and a quote or newline in it is looked for
    /// in its escaped form. This is the plain scan the log search starts from; the caller drops
    /// the kinds of event it does not want and asks again from the last `seq` it saw.
    pub fn matching(
        &self,
        terms: &[&str],
        before: Option<Seq>,
        limit: usize,
    ) -> Result<Vec<Envelope<S::Event>>> {
        let mut sql = String::from("SELECT seq, at, by, event FROM events WHERE seq < ?1");
        let mut args: Vec<String> = Vec::new();
        for (i, term) in terms.iter().enumerate() {
            // Escaped for JSON first, then for LIKE, whose wildcards would otherwise match text.
            let json = serde_json::to_string(term).expect("a string serialises");
            let json = &json[1..json.len() - 1];
            let like = json
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            sql.push_str(&format!(" AND event LIKE ?{} ESCAPE '\\'", i + 3));
            args.push(format!("%{like}%"));
        }
        sql.push_str(" ORDER BY seq DESC LIMIT ?2");
        let sh = self.shared();
        let mut stmt = sh.conn.prepare(&sql)?;
        let mut params: Vec<rusqlite::types::Value> = vec![
            before.map_or(i64::MAX, |s| s.0 as i64).into(),
            (limit as i64).into(),
        ];
        params.extend(args.into_iter().map(Into::into));
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, at, by, event) = row?;
            let bad = |source| Error::Undecodable {
                table: "events",
                row: seq as u64,
                source,
            };
            out.push(Envelope {
                seq: Seq(seq as u64),
                at: x100_core::time::Timestamp::from_millis(at as u64),
                by: serde_json::from_str(&by).map_err(bad)?,
                event: serde_json::from_str(&event).map_err(bad)?,
            });
        }
        Ok(out)
    }

    /// The state now, and a receiver of every event appended after it.
    pub fn snapshot_and_follow(&self) -> (S, Follow<S::Event>) {
        let mut sh = self.shared();
        let (tx, rx) = unbounded_channel();
        sh.followers.push(tx);
        (sh.state.clone(), rx)
    }

    /// A receiver of every event after `after` (all of them for `None`): the stored ones first,
    /// then new ones as they are appended. For a client that remembers where it got to.
    pub fn follow_after(&self, after: Option<Seq>) -> Result<Follow<S::Event>> {
        let mut sh = self.shared();
        let (tx, rx) = unbounded_channel();
        let mut from = after;
        loop {
            let page = read::<S::Event>(&sh.conn, Dir::After(from), 1000)?;
            let Some(last) = page.last().map(|e| e.seq) else {
                break;
            };
            from = Some(last);
            for e in page {
                // The receiver is in our hands, so this cannot fail.
                let _ = tx.send(e);
            }
        }
        sh.followers.push(tx);
        Ok(rx)
    }
}

enum Dir {
    After(Option<Seq>),
    Before(Option<Seq>),
}

fn insert<E: Serialize>(conn: &mut Connection, events: &[Envelope<E>]) -> Result<()> {
    let tx = conn.transaction()?;
    for e in events {
        let by = serde_json::to_string(&e.by).expect("an actor serialises");
        let event = serde_json::to_string(&e.event).expect("an event serialises");
        tx.execute(
            "INSERT INTO events (seq, at, by, event) VALUES (?1, ?2, ?3, ?4)",
            params![e.seq.0 as i64, e.at.as_millis() as i64, by, event],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn read<E: DeserializeOwned>(
    conn: &Connection,
    dir: Dir,
    limit: usize,
) -> Result<Vec<Envelope<E>>> {
    let (sql, bound) = match dir {
        Dir::After(a) => (
            "SELECT seq, at, by, event FROM events WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
            a.map_or(0, |s| s.0) as i64,
        ),
        Dir::Before(b) => (
            "SELECT seq, at, by, event FROM events WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2",
            b.map_or(i64::MAX, |s| s.0 as i64),
        ),
    };
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params![bound, limit as i64], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, at, by, event) = row?;
        let bad = |source| Error::Undecodable {
            table: "events",
            row: seq as u64,
            source,
        };
        out.push(Envelope {
            seq: Seq(seq as u64),
            at: x100_core::time::Timestamp::from_millis(at as u64),
            by: serde_json::from_str(&by).map_err(bad)?,
            event: serde_json::from_str(&event).map_err(bad)?,
        });
    }
    Ok(out)
}

fn fold_stored<S: Folded>(conn: &Connection) -> Result<S> {
    let mut state = S::default();
    let mut from = None;
    loop {
        let page = read::<S::Event>(conn, Dir::After(from), 1000)?;
        let Some(last) = page.last().map(|e| e.seq) else {
            return Ok(state);
        };
        from = Some(last);
        for e in &page {
            state.apply(e).map_err(|e| Error::Corrupt(Box::new(e)))?;
        }
    }
}
