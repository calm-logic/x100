//! Waits ending, and the inbox items that wake an agent.
//!
//! `Wait::ended_by` in core says which events end which waits. This applies it: each event a
//! writer appends is checked against every open wait, and a wait it ends is closed, its deadline
//! job cancelled, and its agent told, all in the same append. Nothing else closes a wait, so the
//! rule has one home.

use std::collections::BTreeSet;

use x100_core::agent::AgentEvent;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, InboxItemId, WaitId};
use x100_core::inbox::InboxItem;
use x100_core::job::{JobEvent, JobState, ProjectJob};
use x100_core::party::Actor;
use x100_core::task::TaskState;
use x100_core::time::Clock;
use x100_core::wait::{Wait, WaitEnd, WaitOn};

use crate::ctx::Events;
use crate::ids::new_id;

/// The event that puts `what` in `agent`'s inbox. Anything new wakes a waiting or idle agent.
pub(crate) fn deliver(clock: &dyn Clock, agent: AgentId, what: InboxItem) -> (Actor, ProjectEvent) {
    let item = new_id(clock, InboxItemId::new);
    (
        Actor::Daemon,
        ProjectEvent::Agent {
            agent,
            event: AgentEvent::Delivered { item, what },
        },
    )
}

/// The waits `batch` ends, as the events that close them. `state` is the state before the batch.
pub(crate) fn settle(state: &ProjectState, batch: &Events, clock: &dyn Clock) -> Events {
    let mut ended = BTreeSet::new();
    let mut out = Vec::new();
    for (_, event) in batch {
        for (agent, wait) in open_waits(state) {
            if ended.contains(&wait.id) {
                continue;
            }
            if let Some(end) = wait.ended_by(event) {
                ended.insert(wait.id);
                let deadline_fired = matches!(
                    event,
                    ProjectEvent::Job { job, event: JobEvent::Fired { .. } } if *job == wait.deadline_job
                );
                let how = Close {
                    end,
                    cancel_deadline: !deadline_fired,
                    always_tell: false,
                };
                close(state, agent, wait, how, clock, &mut out);
            }
        }
    }
    out
}

/// Waits whose end is already in the log: what a daemon that stopped between an event and its
/// reaction left open. Run when a project is attached.
pub(crate) fn reconcile(state: &ProjectState, clock: &dyn Clock) -> Events {
    let mut out = Vec::new();
    for (agent, wait) in open_waits(state) {
        if let Some(end) = already_over(state, wait) {
            // What woke it may be what the stopped daemon never wrote, so it is told either way.
            let how = Close {
                end,
                cancel_deadline: true,
                always_tell: true,
            };
            close(state, agent, wait, how, clock, &mut out);
        }
    }
    out
}

/// How a wait would end if it were opened now: `Some` if what it waits for has happened or can no
/// longer happen. `wait` refuses such a wait rather than open one that nothing will close.
pub(crate) fn already_over(state: &ProjectState, wait: &Wait) -> Option<WaitEnd> {
    if matches!(
        state.job(wait.deadline_job).map(|j| j.state),
        Some(JobState::Done)
    ) {
        return Some(WaitEnd::Expired);
    }
    match wait.on {
        WaitOn::Job(job) => match state.job(job).map(|j| j.state) {
            None | Some(JobState::Cancelled) => Some(WaitEnd::CannotHappen),
            Some(JobState::Done) => Some(WaitEnd::Satisfied),
            Some(JobState::Scheduled { .. }) => None,
        },
        WaitOn::Report(task) => match state.task(task).map(|t| &t.state) {
            None => Some(WaitEnd::CannotHappen),
            Some(TaskState::InProgress(_) | TaskState::NewIdea) => None,
            Some(_) => Some(WaitEnd::Satisfied),
        },
        WaitOn::Reply { message, .. } => match state.message(message) {
            None => Some(WaitEnd::CannotHappen),
            Some(m) if m.reply.is_some() => Some(WaitEnd::Satisfied),
            Some(_) => None,
        },
        WaitOn::Decision(action) => match state.action(action).map(|a| a.state) {
            None => Some(WaitEnd::CannotHappen),
            Some(x100_core::action::ActionState::Held) => None,
            Some(_) => Some(WaitEnd::Satisfied),
        },
    }
}

fn open_waits(state: &ProjectState) -> impl Iterator<Item = (AgentId, &Wait)> {
    state
        .agents()
        .flat_map(|a| a.waits.values().map(move |w| (a.id, w)))
}

fn job_scheduled(state: &ProjectState, wait: &Wait) -> bool {
    matches!(
        state.job(wait.deadline_job).map(|j| j.state),
        Some(JobState::Scheduled { .. })
    )
}

struct Close {
    end: WaitEnd,
    /// False when the deadline is what ended it, since a job that fired cannot be cancelled.
    cancel_deadline: bool,
    always_tell: bool,
}

fn close(
    state: &ProjectState,
    agent: AgentId,
    wait: &Wait,
    how: Close,
    clock: &dyn Clock,
    out: &mut Events,
) {
    let Close {
        end,
        cancel_deadline,
        always_tell,
    } = how;
    out.push((
        Actor::Daemon,
        ProjectEvent::Agent {
            agent,
            event: AgentEvent::WaitEnded { wait: wait.id, end },
        },
    ));
    if cancel_deadline && job_scheduled(state, wait) {
        out.push((
            Actor::Daemon,
            ProjectEvent::Job {
                job: wait.deadline_job,
                event: JobEvent::Cancelled { by: Actor::Daemon },
            },
        ));
    }
    if always_tell || !woken_by_what_it_waited_for(state, agent, wait, end) {
        out.push(deliver(clock, agent, ended_item(wait.id, end)));
    }
}

fn ended_item(wait: WaitId, end: WaitEnd) -> InboxItem {
    InboxItem::WaitEnded { wait, end }
}

/// A report, a reply and a job that wakes the waiter each reach its inbox themselves. Anything
/// else that ends a wait would leave the agent asleep, so it is told in an item of its own.
fn woken_by_what_it_waited_for(
    state: &ProjectState,
    agent: AgentId,
    wait: &Wait,
    end: WaitEnd,
) -> bool {
    if end != WaitEnd::Satisfied {
        return false;
    }
    match wait.on {
        WaitOn::Report(_) | WaitOn::Reply { .. } => true,
        WaitOn::Job(job) => matches!(
            state.job(job).map(|j| &j.job.does),
            Some(ProjectJob::Wake { agent: woken, .. }) if *woken == agent
        ),
        WaitOn::Decision(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use x100_core::fold::ProjectState;
    use x100_core::job::JobEvent;
    use x100_core::message::{Message, Route};
    use x100_core::testing::fernleaf;
    use x100_core::time::{FakeClock, Timestamp};

    use super::*;

    /// Fernleaf ends with the CEO waiting on its second question to the builder.
    fn open_question() -> (ProjectState, AgentId, Wait) {
        let f = fernleaf();
        let state = ProjectState::fold(&f.log).unwrap();
        let wait = state
            .agent(f.ceo)
            .unwrap()
            .waits
            .values()
            .find(|w| matches!(w.on, WaitOn::Reply { .. }))
            .expect("the CEO waits on the builder's answer")
            .clone();
        (state, f.ceo, wait)
    }

    fn kinds(events: &Events) -> Vec<String> {
        events
            .iter()
            .map(|(_, e)| match e {
                ProjectEvent::Agent { event, .. } => format!("Agent.{}", event.name()),
                ProjectEvent::Job { event, .. } => format!("Job.{}", event.name()),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_reply_ends_the_wait_on_it_and_cancels_its_deadline() {
        let (state, _, wait) = open_question();
        let WaitOn::Reply { message, .. } = wait.on else {
            panic!("a wait on a reply")
        };
        let reply = ProjectEvent::MessageSent(Message {
            id: x100_core::id::MessageId::from_u128(900),
            route: Route::BuilderToCeo,
            body: "Yes.".into(),
            attachments: vec![],
            in_reply_to: Some(message),
        });
        let clock = FakeClock::new(Timestamp::from_millis(1));
        let ended = settle(&state, &vec![(Actor::Builder, reply)], &clock);
        // The reply reaches the CEO's inbox itself, so no item says the wait ended.
        assert_eq!(kinds(&ended), ["Agent.WaitEnded", "Job.Cancelled"]);
    }

    #[test]
    fn a_deadline_that_fires_ends_its_wait_and_says_so() {
        let (state, ceo, wait) = open_question();
        let fired = ProjectEvent::Job {
            job: wait.deadline_job,
            event: JobEvent::Fired { at: wait.deadline },
        };
        let clock = FakeClock::new(wait.deadline);
        let ended = settle(&state, &vec![(Actor::Daemon, fired)], &clock);
        assert_eq!(kinds(&ended), ["Agent.WaitEnded", "Agent.Delivered"]);
        assert!(matches!(
            &ended[1].1,
            ProjectEvent::Agent { agent, event: AgentEvent::Delivered { what: InboxItem::WaitEnded { end: WaitEnd::Expired, .. }, .. } }
                if *agent == ceo
        ));
    }

    #[test]
    fn events_that_end_no_wait_end_none_and_a_settled_log_has_nothing_to_reconcile() {
        let (state, _, _) = open_question();
        let clock = FakeClock::new(Timestamp::from_millis(1));
        let unrelated = ProjectEvent::PreviewSet {
            url: "http://x".into(),
        };
        assert!(settle(&state, &vec![(Actor::Daemon, unrelated)], &clock).is_empty());
        assert!(reconcile(&state, &clock).is_empty());
    }
}
