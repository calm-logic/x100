//! One project as the orchestrator holds it: its store, and the lock every writer takes.
//!
//! A tool call, a turn's end and a job's firing each read the folded state, decide, and append.
//! Without one lock a decision could rest on a state that changed before its events were written:
//! a wait opened just after the report it waits for would wait until its deadline. So every
//! append the orchestrator makes goes through [`Writer`], which also ends the waits the new events
//! end, in the same append. The job clock appends a job's firing on its own, and the reaction to
//! it comes through here like everything else.

use std::sync::{Arc, Mutex, MutexGuard};

use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::id::ProjectId;
use x100_core::party::Actor;
use x100_core::time::Clock;
use x100_store::ProjectStore;

use crate::settle::settle;

pub(crate) type Events = Vec<(Actor, ProjectEvent)>;

pub(crate) struct ProjectCtx {
    pub id: ProjectId,
    pub store: ProjectStore,
    pub clock: Arc<dyn Clock>,
    lock: Mutex<()>,
}

impl ProjectCtx {
    pub fn new(id: ProjectId, store: ProjectStore, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            id,
            store,
            clock,
            lock: Mutex::new(()),
        })
    }

    /// Takes the lock. Blocks, so it is called on a blocking thread.
    pub fn writer(&self) -> Writer<'_> {
        Writer {
            ctx: self,
            _held: self.lock.lock().expect("project writer lock"),
        }
    }

    /// Reads, decides and appends under the lock. Blocks.
    pub fn commit<R>(
        &self,
        decide: impl FnOnce(&ProjectState, &dyn Clock) -> (Events, R),
    ) -> x100_store::Result<R> {
        let w = self.writer();
        let (events, r) = w.read(|s| decide(s, &*self.clock));
        w.append(events)?;
        Ok(r)
    }
}

/// The lock, held. Reads and appends made through it see no other writer's events in between.
pub(crate) struct Writer<'a> {
    ctx: &'a ProjectCtx,
    _held: MutexGuard<'a, ()>,
}

impl Writer<'_> {
    pub fn read<T>(&self, f: impl FnOnce(&ProjectState) -> T) -> T {
        self.ctx.store.log.with_state(f)
    }

    pub fn clock(&self) -> &dyn Clock {
        &*self.ctx.clock
    }

    /// Appends `events` as one unit, with the end of every wait they end.
    pub fn append(&self, mut events: Events) -> x100_store::Result<Vec<Envelope<ProjectEvent>>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let ended = self.read(|s| settle(s, &events, &*self.ctx.clock));
        events.extend(ended);
        self.ctx.store.log.append_all(events)
    }
}
