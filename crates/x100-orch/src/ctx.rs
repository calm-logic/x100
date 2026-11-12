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
use x100_jobs::Scheduler;
use x100_store::{Git, ProjectStore};

use crate::settle::settle;

pub(crate) type Events = Vec<(Actor, ProjectEvent)>;

pub(crate) struct ProjectCtx {
    pub id: ProjectId,
    pub store: ProjectStore,
    pub clock: Arc<dyn Clock>,
    /// The project's job clock, which learns of a job only from `schedule` or a wake.
    jobs: Scheduler<ProjectState>,
    lock: Mutex<()>,
}

impl ProjectCtx {
    pub fn new(
        id: ProjectId,
        store: ProjectStore,
        clock: Arc<dyn Clock>,
        jobs: Scheduler<ProjectState>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            store,
            clock,
            jobs,
            lock: Mutex::new(()),
        })
    }

    /// Git for the project's code directory, once it has one. A new project has none until its
    /// CEO sets it up.
    pub fn git(&self) -> Option<Git> {
        self.store.log.with_state(git_of)
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

    /// Appends `events` as one unit, with the end of every wait they end, and wakes the job clock
    /// for any job among them.
    pub fn append(&self, mut events: Events) -> x100_store::Result<Vec<Envelope<ProjectEvent>>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let ended = self.read(|s| settle(s, &events, &*self.ctx.clock));
        events.extend(ended);
        let scheduled = events
            .iter()
            .any(|(_, e)| matches!(e, ProjectEvent::JobScheduled(_)));
        let appended = self.ctx.store.log.append_all(events)?;
        if scheduled {
            self.ctx.jobs.wake();
        }
        Ok(appended)
    }
}

/// Git for the code directory `state` records, if it records one.
pub(crate) fn git_of(state: &ProjectState) -> Option<Git> {
    state
        .project()
        .and_then(|p| p.code_dir.as_ref())
        .map(|dir| Git::new(dir.clone()))
}
