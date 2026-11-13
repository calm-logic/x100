//! What the start and the end of a turn write, decided from the folded state.
//!
//! A turn that ends with its agent's task neither reported nor waited on is followed by another,
//! through a nudge in the inbox, and after the daemon's set number of such turns in a row the task
//! is reported failed. The count rides on the nudge itself, so it survives a change of driver.

use std::collections::BTreeSet;

use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::{Driver, DriverEvent, PlannedReason};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::git::CommitId;
use x100_core::id::{AgentId, DriverId, InboxItemId, JobId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::job::{Job, JobEvent, JobState, ProjectJob, ProjectRoutine, Schedule};
use x100_core::party::Actor;
use x100_core::task::TaskFailure;
use x100_core::time::{Clock, Timestamp};

use x100_core::driver::RetireReason;
use x100_router::{Upkeep, Warm};

use crate::Shared;
use crate::ctx::Events;
use crate::ids::new_id;
use crate::route;
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

/// What a turn that completed leaves to record.
pub(crate) struct Close {
    pub agent: AgentId,
    pub driver: DriverId,
    pub record: Option<Driver>,
    /// Every task the driver has had a turn on.
    pub carried: BTreeSet<TaskId>,
    pub given: Vec<InboxItemId>,
    pub checkpoint: Option<CommitId>,
    pub limit: u32,
    /// The driver works in the scratch directory, from before the project had code.
    pub scratch: bool,
}

/// What becomes of a driver whose turn completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum After {
    /// It is warm, for its agent's next turn.
    Warm,
    /// The router ended it with the turn.
    Retired,
    /// Its agent is mid-task and it should not go on: it is handing off, and its last turn
    /// follows at once.
    HandOff(PlannedReason),
}

/// The end of a turn, and what becomes of its driver. Decided in the same append as the turn's
/// end, so the log never shows warm a driver the router had already ended. A driver whose agent
/// carries on with its task, and that should not go on, hands off instead of retiring: its turn
/// may have left the progress note behind the work, and a last turn brings it up to date.
pub(crate) fn close(
    shared: &Shared,
    state: &ProjectState,
    clock: &dyn Clock,
    c: Close,
) -> (Events, After) {
    let warm = Warm {
        driver: c.driver,
        carried: c.carried,
        accepted: false,
    };
    let set_up = state.project().is_some_and(|p| p.code_dir.is_some());
    let carries_on = carries_on(state, c.agent, &c.given, c.limit);
    let hand_off = carries_on
        .then(|| route::hand_off_in(shared, state, c.driver, clock.now()))
        .flatten();
    let end: Option<RetireReason> = match route::upkeep_in(shared, state, &warm, clock.now()) {
        // It cannot move into the code directory, so the agent's next turn is on a new driver.
        _ if c.scratch && set_up => Some(RetireReason::ProjectSetUp),
        _ if hand_off.is_some() => None,
        Upkeep::End(why) => Some(why),
        Upkeep::Keep => None,
    };
    let warm_until = match (end.is_some() || hand_off.is_some(), &c.record) {
        (true, _) => None,
        (false, Some(r)) => Some(
            shared
                .daemon
                .with_state(|d| x100_router::warm_until(d, r, clock.now())),
        ),
        (false, None) => Some(clock.now() + x100_router::DEFAULT_WARM),
    };
    let ending = Ending {
        agent: c.agent,
        driver: c.driver,
        given: c.given,
        checkpoint: c.checkpoint,
        warm_until,
        limit: c.limit,
    };
    let mut events = ended(state, clock, ending);
    let driver = c.driver;
    let after = match (end, hand_off) {
        (Some(why), _) => {
            let event = DriverEvent::Retired(why);
            events.push((Actor::Daemon, ProjectEvent::Driver { driver, event }));
            After::Retired
        }
        (None, Some(reason)) => {
            let event = DriverEvent::HandoffStarted(reason);
            events.push((Actor::Daemon, ProjectEvent::Driver { driver, event }));
            After::HandOff(reason)
        }
        (None, None) => After::Warm,
    };
    (events, after)
}

/// The end of a planned handoff's last turn: the work checkpointed and the driver ended. Its
/// agent's items wait for the next driver, and the turn is not counted as one without progress,
/// since all it was asked to do was write its note.
pub(crate) fn last_turn_ended(
    state: &ProjectState,
    agent: AgentId,
    driver: DriverId,
    checkpoint: Option<CommitId>,
) -> Events {
    let mut out: Events = checkpoint
        .map(|commit| (Actor::Daemon, ProjectEvent::Checkpointed { agent, commit }))
        .into_iter()
        .collect();
    out.push((
        Actor::Daemon,
        ProjectEvent::Driver {
            driver,
            event: DriverEvent::TurnEnded,
        },
    ));
    if let Some(job) = warm_period_job(state, driver) {
        let event = JobEvent::Cancelled { by: Actor::Daemon };
        out.push((Actor::Daemon, ProjectEvent::Job { job, event }));
    }
    out
}

/// Whether the agent carries on with its task after a turn given `given`: it has the task open,
/// neither reported nor waited on, and is not yet past the limit of such turns.
fn carries_on(state: &ProjectState, agent: AgentId, given: &[InboxItemId], limit: u32) -> bool {
    unfinished(state, agent).is_some_and(|task| nudged(state, agent, given, task) + 1 < limit)
}

/// The agent's current task if a turn that just ended left it neither reported nor waited on. A
/// task that has not reached the agent yet, such as the one setting the project up just opened,
/// brings its own turn, so it is not one of these.
fn unfinished(state: &ProjectState, agent: AgentId) -> Option<TaskId> {
    let a = state.agent(agent)?;
    let task = state.current_task(agent)?.task.id;
    let to_come = a
        .inbox
        .waiting()
        .any(|i| a.items.get(&i) == Some(&InboxItem::Task { task }));
    (a.waits.is_empty() && !to_come).then_some(task)
}

/// A turn that completed.
pub(crate) struct Ending {
    pub agent: AgentId,
    pub driver: DriverId,
    pub given: Vec<InboxItemId>,
    pub checkpoint: Option<CommitId>,
    /// When the driver's warm period ends: its model's cache lifetime from now. `None` when it
    /// ends with this turn.
    pub warm_until: Option<Timestamp>,
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
    out.push((
        Actor::Daemon,
        ProjectEvent::Driver {
            driver: e.driver,
            event: DriverEvent::TurnEnded,
        },
    ));
    if let Some(a) = state.agent(e.agent) {
        let mut closed: Option<TaskId> = None;
        if let Some(task) = unfinished(state, e.agent) {
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
        // An agent woken from its waits that did not wait again still has them open.
        let next = if !a.waits.is_empty() {
            Some(AgentEvent::StillWaiting)
        } else {
            (!open).then_some(AgentEvent::WentIdle)
        };
        if let Some(event) = next.filter(|_| a.state == AgentState::Working && nothing_waiting) {
            out.push((
                Actor::Daemon,
                ProjectEvent::Agent {
                    agent: e.agent,
                    event,
                },
            ));
        }
    }
    if let Some(at) = e.warm_until {
        let job = Job {
            id: new_id(clock, JobId::new),
            when: Schedule::Once { at },
            does: ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver: e.driver }),
        };
        out.push((Actor::Daemon, ProjectEvent::JobScheduled(job)));
    }
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
