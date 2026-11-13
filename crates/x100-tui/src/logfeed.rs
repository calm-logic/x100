//! The Log tab's lines, and the same lines for the `log` command. The daemon's log is read a page
//! at a time, newest first, and followed by reading what came after the newest event held.

use std::collections::{HashMap, VecDeque};

use x100_client::wording::{self, Names};
use x100_client::words;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::FileId;

pub const PAGE: u32 = 200;
pub const CATCH_UP: u32 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRow {
    pub seq: Seq,
    pub at: String,
    pub who: String,
    pub text: String,
}

/// Words for `events`, oldest first. `files` learns the name of every file an event adds, so a
/// removal read later can still name it.
pub fn rows(
    events: &[Envelope<ProjectEvent>],
    p: &ProjectState,
    d: &DaemonState,
    files: &mut HashMap<FileId, String>,
) -> Vec<LogRow> {
    for e in events {
        if let ProjectEvent::FileAdded(f) = &e.event {
            files.insert(f.id, f.name.clone());
        }
    }
    let names = Names {
        project: p,
        daemon: d,
        files,
    };
    events
        .iter()
        .map(|e| {
            let l = wording::line(e, &names);
            LogRow {
                seq: e.seq,
                at: words::moment(e.at),
                who: l.who,
                text: l.text,
            }
        })
        .collect()
}

/// What to ask the daemon for next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    First,
    After(Seq),
    Before(Seq),
}

#[derive(Debug, Default)]
pub struct LogFeed {
    /// Newest first.
    pub lines: VecDeque<LogRow>,
    newest: Option<Seq>,
    oldest: Option<Seq>,
    older: bool,
    loading: bool,
    files: HashMap<FileId, String>,
}

impl LogFeed {
    pub fn newest(&self) -> Option<Seq> {
        self.newest
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The next read, given the newest event the client holds, if any is needed.
    pub fn next_read(&mut self, held: Option<Seq>, wants_older: bool) -> Option<Read> {
        if self.loading {
            return None;
        }
        let read = match (self.newest, self.oldest) {
            (None, _) if held.is_some() => Read::First,
            (Some(n), _) if held > Some(n) => Read::After(n),
            (_, Some(o)) if self.older && wants_older => Read::Before(o),
            _ => return None,
        };
        self.loading = true;
        Some(read)
    }

    pub fn failed(&mut self) {
        self.loading = false;
    }

    pub fn reset(&mut self) {
        *self = LogFeed::default();
    }

    /// Takes in a page read for `read`, as the daemon returned it.
    pub fn ingest(
        &mut self,
        read: Read,
        mut events: Vec<Envelope<ProjectEvent>>,
        p: &ProjectState,
        d: &DaemonState,
    ) {
        self.loading = false;
        let forward = matches!(read, Read::After(_));
        if !forward {
            events.reverse();
            let limit = PAGE as usize;
            self.older = events.len() == limit && events.first().is_some_and(|e| e.seq > Seq(1));
        }
        let made = rows(&events, p, d, &mut self.files);
        let (Some(first), Some(last)) = (made.first().map(|r| r.seq), made.last().map(|r| r.seq))
        else {
            return;
        };
        if forward {
            self.newest = Some(last);
            self.oldest.get_or_insert(first);
            for r in made {
                self.lines.push_front(r);
            }
        } else {
            self.oldest = Some(first);
            self.newest.get_or_insert(last);
            for r in made.into_iter().rev() {
                self.lines.push_back(r);
            }
        }
    }
}
