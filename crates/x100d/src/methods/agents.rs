use x100_drivers::terminal::Size;
use x100_orch::DriverError;
use x100_proto::{
    CancelJob, DecideAction, Done, ErrorCode, Inbox, InboxEntry, ReadInbox, ReadStartPack,
    ReadWaits, ResizeTerminal, RpcError, SetEffortRange, SetLeadBudget, StartPack, StopDriver,
    Waits,
};

use super::{project, refused, terminal_of};
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

/// Through the orchestrator, which carries out what the decision calls for and tells the agent.
pub async fn decide(hub: &Hub, p: DecideAction) -> Result<Done, RpcError> {
    project(hub, p.project)?;
    hub.orch
        .decide(p.project, p.action, p.decision)
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
        return Err(ended());
    }
    hub.orch
        .stop_driver(p.project, p.driver)
        .await
        .map_err(driver_error)?;
    Ok(Done {})
}

/// Through the orchestrator to the driver's own task, which owns its terminal, so the resize lands
/// in the recording between two reads of the program's output.
pub async fn resize_terminal(hub: &Hub, p: ResizeTerminal) -> Result<Done, RpcError> {
    let store = terminal_of(hub, p.project, p.driver)?;
    let (min, max) = (ResizeTerminal::MIN, ResizeTerminal::MAX);
    if !(min.0..=max.0).contains(&p.cols) || !(min.1..=max.1).contains(&p.rows) {
        return Err(RpcError::new(
            ErrorCode::Rejected,
            format!(
                "a terminal is {} by {} to {} by {} cells, not {} by {}",
                min.0, min.1, max.0, max.1, p.cols, p.rows
            ),
        ));
    }
    let live = store
        .log
        .with_state(|s| s.driver(p.driver).is_some_and(|d| d.state.is_live()));
    if !live {
        return Err(ended());
    }
    let size = Size {
        cols: p.cols,
        rows: p.rows,
    };
    hub.orch
        .resize_terminal(p.project, p.driver, size)
        .await
        .map_err(driver_error)?;
    Ok(Done {})
}

fn ended() -> RpcError {
    RpcError::new(ErrorCode::WrongState, "the driver has ended already")
}

fn driver_error(e: DriverError) -> RpcError {
    match e {
        DriverError::NotFound => RpcError::new(ErrorCode::NotFound, e.to_string()),
        DriverError::NotLive => RpcError::new(ErrorCode::WrongState, e.to_string()),
    }
}

/// Through the orchestrator, so the change is checked and appended under the project's lock, and
/// the next decision for the lead's agents reads it.
pub async fn set_budget(hub: &Hub, p: SetLeadBudget) -> Result<Done, RpcError> {
    project(hub, p.project)?;
    hub.orch
        .set_budget(p.project, p.lead, p.budget)
        .await
        .map_err(refused)?;
    Ok(Done {})
}

pub async fn set_effort(hub: &Hub, p: SetEffortRange) -> Result<Done, RpcError> {
    project(hub, p.project)?;
    hub.orch
        .set_effort(p.project, p.agent, p.range)
        .await
        .map_err(refused)?;
    Ok(Done {})
}
