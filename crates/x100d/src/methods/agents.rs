use x100_orch::StopError;
use x100_proto::{
    CancelJob, Done, ErrorCode, Inbox, InboxEntry, ReadInbox, ReadStartPack, ReadWaits, RpcError,
    StartPack, StopDriver, Waits,
};

use super::{project, refused};
use crate::hub::Hub;

pub async fn cancel_job(hub: &Hub, p: CancelJob) -> Result<Done, RpcError> {
    project(hub, p.project)?;
    // Through the orchestrator, so a wait on the job ends with it.
    hub.orch
        .cancel_job(p.project, p.job)
        .await
        .map_err(refused)?;
    Ok(Done {})
}

pub fn waits(hub: &Hub, p: ReadWaits) -> Result<Waits, RpcError> {
    project(hub, p.project)?;
    hub.orch
        .waits(p.project, p.agent)
        .map(|waits| Waits { waits })
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, format!("no agent {}", p.agent)))
}

pub fn start_pack(hub: &Hub, p: ReadStartPack) -> Result<StartPack, RpcError> {
    project(hub, p.project)?;
    hub.orch
        .start_pack(p.project, p.driver)
        .map(|text| StartPack { text })
        .ok_or_else(|| {
            RpcError::new(
                ErrorCode::NotFound,
                format!("no start pack for driver {}", p.driver),
            )
        })
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
