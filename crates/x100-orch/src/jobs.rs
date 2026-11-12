//! What a job that fired does in a project. The job clock has already written the firing; this is
//! the reaction, appended after it: the waits it ends, the note it delivers, the fresh task it
//! delegates, the tasks given up when a wait for a driver runs out. Ending a warm period stops a
//! process, so the turn loop does that part.

use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::TaskId;
use x100_core::inbox::InboxItem;
use x100_core::job::{Job, JobEvent, ProjectJob, ProjectRoutine};
use x100_core::party::{Actor, Party};
use x100_core::task::{Assignment, Opening, Task};
use x100_core::time::{Clock, Timestamp};

use crate::ctx::Events;
use crate::ids::new_id;
use crate::route;
use crate::settle::{deliver, settle};

pub(crate) fn fired(
    state: &ProjectState,
    clock: &dyn Clock,
    job: &Job<ProjectJob>,
    at: Timestamp,
) -> Events {
    let firing = ProjectEvent::Job {
        job: job.id,
        event: JobEvent::Fired { at },
    };
    let mut out = settle(state, &vec![(Actor::Daemon, firing)], clock);
    let Some(team) = state.project().map(|p| &p.team) else {
        return out;
    };
    match &job.does {
        ProjectJob::Wake { agent, note, .. } => {
            if !team.is_retired(*agent) && state.agent(*agent).is_some() {
                let what = InboxItem::Job {
                    job: job.id,
                    note: note.clone(),
                };
                out.push(deliver(clock, *agent, what));
            }
        }
        // As if the agent that scheduled it delegated a fresh task, so only while the assignee
        // is still one of its agents.
        ProjectJob::Delegate {
            scheduled_by,
            to,
            title,
            ask,
            effort,
        } => {
            let still_its = team.parent(*to) == Some(*scheduled_by) && !team.is_retired(*to);
            if let (true, Some(range)) = (still_its, team.effort(*to)) {
                let id = new_id(clock, TaskId::new);
                let task = Task {
                    id,
                    title: title.clone(),
                    ask: ask.clone(),
                    part: None,
                    raised_by: Party::Agent(*scheduled_by),
                    attachments: Vec::new(),
                };
                let opening = Opening::Delegated(Assignment {
                    delegator: Party::Agent(*scheduled_by),
                    assignee: *to,
                    effort: range.clamp(*effort),
                });
                out.push((
                    Actor::Agent(*scheduled_by),
                    ProjectEvent::TaskOpened { task, opening },
                ));
                out.push(deliver(clock, *to, InboxItem::Task { task: id }));
            }
        }
        ProjectJob::Routine(ProjectRoutine::DriverWaitDeadline { agent }) => {
            out.extend(route::deadline_passed(state, clock, *agent));
        }
        // A wait's own deadline is closed by `settle`, like any other event that ends a wait.
        ProjectJob::Routine(
            ProjectRoutine::WaitDeadline { .. } | ProjectRoutine::EndWarmPeriod { .. },
        ) => {}
    }
    out
}
