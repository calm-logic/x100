//! What the start and the end of a turn write, decided from the folded state.
//!
//! A turn that ends with its agent's task neither reported nor waited on is followed by another,
//! through a nudge in the inbox, and after the daemon's set number of such turns in a row the task
//! is reported failed. The count rides on the nudge itself, so it survives a change of driver.

use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::DriverEvent;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::git::CommitId;
use x100_core::id::{AgentId, DriverId, InboxItemId, JobId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::job::{Job, JobEvent, JobState, ProjectJob, ProjectRoutine, Schedule};
use x100_core::message::Message;
use x100_core::party::Actor;
use x100_core::task::TaskFailure;
use x100_core::time::{Clock, Timestamp};

use crate::ctx::Events;
use crate::ids::new_id;
use crate::settle::deliver;
use crate::tools::report_failed;

/// The events that begin a turn: the branch refreshes made for it, the end of the driver's warm
/// period being called off, and the turn with the items it is given.
pub(crate) fn started(
    state: &ProjectState,
    driver: DriverId,
    given: Vec<InboxItemId>,
    refreshed: Vec<ProjectEvent>,
) -> Events {
    let mut out: Events = refreshed.into_iter().map(|e| (Actor::Daemon, e)).collect();
    if let Some(job) = warm_period_job(state, driver) {
        out.push((
            Actor::Daemon,
            ProjectEvent::Job {
                job,
                event: JobEvent::Cancelled { by: Actor::Daemon },
            },
        ));
    }
    out.push((
        Actor::Daemon,
        ProjectEvent::Driver {
            driver,
            event: DriverEvent::TurnStarted { given },
        },
    ));
    out
}

/// The job that ends `driver`'s warm period, while it can still fire.
pub(crate) fn warm_period_job(state: &ProjectState, driver: DriverId) -> Option<JobId> {
    state.jobs().find_map(|j| {
        let ours = matches!(
            j.job.does,
            ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver: d }) if d == driver
        );
        (ours && matches!(j.state, JobState::Scheduled { .. })).then_some(j.job.id)
    })
}

/// A turn that completed.
pub(crate) struct Ending {
    pub agent: AgentId,
    pub driver: DriverId,
    pub given: Vec<InboxItemId>,
    pub checkpoint: Option<CommitId>,
    /// The CEO's text, as its reply in the chat.
    pub reply: Option<Message>,
    /// When the driver's warm period ends: its model's cache lifetime from now.
    pub warm_until: Timestamp,
    /// Turns in a row without a report or a wait before the task is reported failed.
    pub limit: u32,
}

pub(crate) fn ended(state: &ProjectState, clock: &dyn Clock, e: Ending) -> Events {
    let mut out = Vec::new();
    if let Some(commit) = e.checkpoint {
        out.push((
            Actor::Daemon,
            ProjectEvent::Checkpointed {
                agent: e.agent,
                commit,
            },
        ));
    }
    if let Some(reply) = e.reply {
        out.push((Actor::Agent(e.agent), ProjectEvent::MessageSent(reply)));
    }
    out.push((
        Actor::Daemon,
        ProjectEvent::Driver {
            driver: e.driver,
            event: DriverEvent::TurnEnded,
        },
    ));
    if let Some(a) = state.agent(e.agent) {
        let mut closed: Option<TaskId> = None;
        let task = state.current_task(e.agent).map(|t| t.task.id);
        if let Some(task) = task.filter(|_| a.waits.is_empty()) {
            let turns = nudged(state, e.agent, &e.given, task) + 1;
            if turns >= e.limit {
                let summary = format!(
                    "{turns} turns in a row ended with the task neither reported nor waited on."
                );
                let failure = TaskFailure::NoProgress { turns };
                out.extend(report_failed(state, clock, task, failure, summary));
                closed = Some(task);
            } else {
                out.push(deliver(clock, e.agent, InboxItem::Nudge { task, turns }));
            }
        }
        let open = a.open_tasks.iter().any(|t| Some(*t) != closed);
        // After `TurnEnded` the inbox holds only what waits for a turn, since this turn's items
        // are then handled.
        let nothing_waiting = a.inbox.waiting().next().is_none();
        if a.state == AgentState::Working && !open && a.waits.is_empty() && nothing_waiting {
            out.push((
                Actor::Daemon,
                ProjectEvent::Agent {
                    agent: e.agent,
                    event: AgentEvent::WentIdle,
                },
            ));
        }
    }
    let job = Job {
        id: new_id(clock, JobId::new),
        when: Schedule::Once { at: e.warm_until },
        does: ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver: e.driver }),
    };
    out.push((Actor::Daemon, ProjectEvent::JobScheduled(job)));
    out
}

/// How many turns in a row had already ended without progress on `task`, as the nudge this turn
/// was given says.
fn nudged(state: &ProjectState, agent: AgentId, given: &[InboxItemId], task: TaskId) -> u32 {
    let Some(a) = state.agent(agent) else {
        return 0;
    };
    given
        .iter()
        .filter_map(|i| match a.items.get(i) {
            Some(InboxItem::Nudge { task: t, turns }) if *t == task => Some(*turns),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}
