//! What the start and the end of a turn write, decided from the folded state.
//!
//! A turn that ends with its agent's task neither reported nor waited on is followed by another,
//! through a nudge in the inbox, and after the daemon's set number of such turns in a row the task
//! is reported failed. The count rides on the nudge itself, so it survives a change of driver.

use std::collections::BTreeSet;

use x100_core::agent::{AgentEvent, AgentState};
use x100_core::driver::{Driver, DriverEvent};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::git::CommitId;
use x100_core::id::{AgentId, DriverId, InboxItemId, JobId, MessageId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::job::{Job, JobEvent, JobState, ProjectJob, ProjectRoutine, Schedule};
use x100_core::message::{Message, Route};
use x100_core::party::Actor;
use x100_core::task::TaskFailure;
use x100_core::time::{Clock, Timestamp};

use x100_router::{EndWhy, Upkeep, Warm};

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
    /// What the driver said, which for the CEO is its reply in the chat.
    pub text: Vec<String>,
    pub reply_to: Option<MessageId>,
    pub checkpoint: Option<CommitId>,
    pub limit: u32,
}

/// The end of a turn, and whether the router ends its driver with it. Decided in the same append
/// as the turn's end, so the log never shows warm a driver the router had already ended.
pub(crate) fn close(
    shared: &Shared,
    state: &ProjectState,
    clock: &dyn Clock,
    c: Close,
) -> (Events, bool) {
    let body = c.text.join("\n\n");
    let is_ceo = state.project().is_some_and(|p| p.team.ceo() == c.agent);
    let reply = (is_ceo && !body.trim().is_empty()).then(|| Message {
        id: new_id(clock, MessageId::new),
        route: Route::CeoToBuilder,
        body,
        attachments: Vec::new(),
        in_reply_to: c.reply_to,
    });
    let warm = Warm {
        driver: c.driver,
        carried: c.carried,
        accepted: false,
    };
    let end = match route::upkeep_in(shared, state, &warm, clock.now()) {
        Upkeep::End(why) => Some(why),
        Upkeep::Keep => None,
    };
    let warm_until = match (&end, &c.record) {
        (Some(_), _) => None,
        (None, Some(r)) => Some(
            shared
                .daemon
                .with_state(|d| x100_router::warm_until(d, r, clock.now())),
        ),
        (None, None) => Some(clock.now() + x100_router::DEFAULT_WARM),
    };
    let ending = Ending {
        agent: c.agent,
        driver: c.driver,
        given: c.given,
        checkpoint: c.checkpoint,
        reply,
        warm_until,
        limit: c.limit,
    };
    let mut events = ended(state, clock, ending);
    for event in end.map(end_events).unwrap_or_default() {
        let driver = c.driver;
        events.push((Actor::Daemon, ProjectEvent::Driver { driver, event }));
    }
    (events, end.is_some())
}

/// How a driver the router ends between turns is recorded. One that no longer fits is recorded as
/// a planned handoff with no last turn: between turns there is nothing left for one to write.
pub(crate) fn end_events(why: EndWhy) -> Vec<DriverEvent> {
    match why {
        EndWhy::Retire(reason) => vec![DriverEvent::Retired(reason)],
        EndWhy::Unfit(reason) => vec![DriverEvent::HandoffStarted(reason), DriverEvent::TurnEnded],
    }
}

/// A turn that completed.
pub(crate) struct Ending {
    pub agent: AgentId,
    pub driver: DriverId,
    pub given: Vec<InboxItemId>,
    pub checkpoint: Option<CommitId>,
    /// The CEO's text, as its reply in the chat.
    pub reply: Option<Message>,
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
