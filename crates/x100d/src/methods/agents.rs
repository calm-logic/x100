use x100_core::job::JobState;
use x100_core::party::Actor;
use x100_orch::StopError;
use x100_proto::{CancelJob, Done, ErrorCode, Inbox, InboxEntry, ReadInbox, RpcError, StopDriver};

use super::{blocking, project};
use crate::hub::Hub;

pub async fn cancel_job(hub: &Hub, p: CancelJob) -> Result<Done, RpcError> {
    let store = project(hub, p.project)?;
    let state = store.log.with_state(|s| s.job(p.job).map(|j| j.state));
    match state {
        None => {
            return Err(RpcError::new(
                ErrorCode::NotFound,
                format!("no job {}", p.job),
            ));
        }
        Some(JobState::Scheduled { .. }) => {}
        Some(other) => {
            return Err(RpcError::new(
                ErrorCode::WrongState,
                format!("job {} is {}, not scheduled", p.job, other.name()),
            ));
        }
    }
    let scheduler = hub
        .scheduler(p.project)
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "no such project"))?;
    // A job that fires between the check and the cancel is refused by the log as a rejected event.
    blocking(move || scheduler.cancel(Actor::Builder, p.job))
        .await
        .map_err(|e| RpcError::new(ErrorCode::WrongState, e.to_string()))?;
    Ok(Done {})
}

pub fn inbox(hub: &Hub, p: ReadInbox) -> Result<Inbox, RpcError> {
    let store = project(hub, p.project)?;
    store
        .log
        .with_state(|s| {
            let a = s.agent(p.agent)?;
            let waiting: Vec<_> = a.inbox.waiting().collect();
            Some(Inbox {
                items: a
                    .inbox
                    .unhandled()
                    .filter_map(|id| {
                        Some(InboxEntry {
                            id,
                            item: a.items.get(&id)?.clone(),
                            being_handled: !waiting.contains(&id),
                        })
                    })
                    .collect(),
            })
        })
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, format!("no agent {}", p.agent)))
}

pub async fn stop_driver(hub: &Hub, p: StopDriver) -> Result<Done, RpcError> {
    let store = project(hub, p.project)?;
    let live = store
        .log
        .with_state(|s| s.driver(p.driver).map(|d| d.state.is_live()))
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, format!("no driver {}", p.driver)))?;
    if !live {
        return Err(RpcError::new(
            ErrorCode::WrongState,
            "the driver has ended already",
        ));
    }
    hub.orch
        .stop_driver(p.project, p.driver)
        .await
        .map_err(|e| match e {
            StopError::NotFound => RpcError::new(ErrorCode::NotFound, e.to_string()),
            StopError::NotLive => RpcError::new(ErrorCode::WrongState, e.to_string()),
        })?;
    Ok(Done {})
}
