//! Waits: what an agent waits for, each with a deadline.
//!
//! A wait always has a deadline. A `wait` call without one gets the default, which is a setting,
//! before the wait is recorded, so no recorded wait can be open forever.

use serde::{Deserialize, Serialize};

use crate::action::ActionEvent;
use crate::agent::AgentEvent;
use crate::event::ProjectEvent;
use crate::id::{ActionId, JobId, MessageId, TaskId, WaitId};
use crate::job::JobEvent;
use crate::party::Party;
use crate::task::TaskEvent;
use crate::time::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Wait {
    pub id: WaitId,
    pub on: WaitOn,
    pub deadline: Timestamp,
    /// The job that fires at the deadline.
    pub deadline_job: JobId,
}

/// What an agent can wait for. A set time is a wait on a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitOn {
    Job(JobId),
    /// The report on a task it delegated.
    Report(TaskId),
    /// A reply to a message it sent, from whoever it was sent to. For the CEO, the builder's
    /// answer.
    Reply {
        message: MessageId,
        from: Party,
    },
    /// The decision on an action it proposed.
    Decision(ActionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitEnd {
    /// What it waited for happened.
    Satisfied,
    /// The deadline came first.
    Expired,
    /// What it waited for can no longer happen.
    CannotHappen,
}

impl Wait {
    /// Whether `event` ends this wait, and how. This is the one place that says which events end
    /// which waits, so the daemon and every client agree.
    pub fn ended_by(&self, event: &ProjectEvent) -> Option<WaitEnd> {
        use ProjectEvent as P;
        if let P::Job {
            job,
            event: JobEvent::Fired { .. },
        } = event
            && *job == self.deadline_job
        {
            return Some(WaitEnd::Expired);
        }
        match (self.on, event) {
            (WaitOn::Job(on), P::Job { job, event }) if on == *job => Some(match event {
                JobEvent::Fired { .. } => WaitEnd::Satisfied,
                JobEvent::Cancelled { .. } => WaitEnd::CannotHappen,
            }),
            (
                WaitOn::Report(on),
                P::Task {
                    task,
                    event: TaskEvent::Reported(_),
                },
            ) if on == *task => Some(WaitEnd::Satisfied),
            (WaitOn::Reply { message, .. }, P::MessageSent(m))
                if m.in_reply_to == Some(message) =>
            {
                Some(WaitEnd::Satisfied)
            }
            (
                WaitOn::Reply {
                    from: Party::Agent(from),
                    ..
                },
                P::Agent {
                    agent,
                    event: AgentEvent::Retired,
                },
            ) if from == *agent => Some(WaitEnd::CannotHappen),
            (
                WaitOn::Decision(on),
                P::Action {
                    action,
                    event: ActionEvent::Allowed | ActionEvent::Approved | ActionEvent::Denied,
                },
            ) if on == *action => Some(WaitEnd::Satisfied),
            _ => None,
        }
    }
}
