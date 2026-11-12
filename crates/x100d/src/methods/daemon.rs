use std::time::Duration;

use x100_core::driver::View;
use x100_core::event::DaemonEvent;
use x100_core::id::{DriverId, ProjectId};
use x100_core::party::Actor;
use x100_core::settings::DaemonSetting;
use x100_proto::{
    Done, ErrorCode, LogId, LogPage, Page, PositionedChunk, PositionedEntry, ReadLog, ReadTerminal,
    ReadTranscript, RescanHarnesses, Rescanned, RpcError, SetClock, SetDaemonSetting,
    SetMachineCap, TerminalPage, TranscriptPage,
};
use x100_store::ProjectStore;

use super::{blocking, project, refused, store_error};
use crate::hub::Hub;

/// The most a page holds, whatever the client asks for.
pub const MAX_PAGE: u32 = 1000;

pub async fn read_log(hub: &Hub, p: ReadLog) -> Result<LogPage, RpcError> {
    let (limit, newest, bound) = match p.page {
        Page::NewestFirst { before, limit } => (limit, true, before),
        Page::OldestFirst { after, limit } => (limit, false, after),
    };
    let limit = limit.min(MAX_PAGE) as usize;
    let read_err = |e| store_error(&e);
    Ok(match p.log {
        LogId::Daemon => {
            let log = hub.log.clone();
            let events = blocking(move || {
                if newest {
                    log.read_before(bound, limit)
                } else {
                    log.read_after(bound, limit)
                }
            })
            .await
            .map_err(read_err)?;
            LogPage::Daemon { events }
        }
        LogId::Project(id) => {
            let log = project(hub, id)?.log;
            let events = blocking(move || {
                if newest {
                    log.read_before(bound, limit)
                } else {
                    log.read_after(bound, limit)
                }
            })
            .await
            .map_err(read_err)?;
            LogPage::Project { events }
        }
    })
}

pub async fn read_transcript(hub: &Hub, p: ReadTranscript) -> Result<TranscriptPage, RpcError> {
    let store = project(hub, p.project)?;
    if store.log.with_state(|s| s.agent(p.agent).is_none()) {
        return Err(RpcError::new(
            ErrorCode::NotFound,
            format!("no agent {}", p.agent),
        ));
    }
    let limit = p.limit.min(MAX_PAGE) as usize;
    let rows = blocking(move || store.transcripts.read(p.agent, p.after, limit))
        .await
        .map_err(|e| store_error(&e))?;
    Ok(TranscriptPage {
        entries: rows
            .into_iter()
            .map(|(position, entry)| PositionedEntry { position, entry })
            .collect(),
    })
}

/// The store of a driver that has a terminal of its own. Any other driver is `not_found`, as the
/// protocol says of a headless one.
pub(crate) fn terminal_of(
    hub: &Hub,
    project_id: ProjectId,
    driver: DriverId,
) -> Result<ProjectStore, RpcError> {
    let store = project(hub, project_id)?;
    match store
        .log
        .with_state(|s| s.driver(driver).map(|d| d.driver.view))
    {
        Some(View::Terminal) => Ok(store),
        Some(View::Headless) => Err(RpcError::new(
            ErrorCode::NotFound,
            format!("driver {driver} runs headless and has no terminal"),
        )),
        None => Err(RpcError::new(
            ErrorCode::NotFound,
            format!("no driver {driver}"),
        )),
    }
}

pub async fn read_terminal(hub: &Hub, p: ReadTerminal) -> Result<TerminalPage, RpcError> {
    let store = terminal_of(hub, p.project, p.driver)?;
    let limit = p.limit.min(MAX_PAGE) as usize;
    let rows = blocking(move || store.terminals.read(p.driver, p.after, limit))
        .await
        .map_err(|e| store_error(&e))?;
    Ok(TerminalPage {
        chunks: rows
            .into_iter()
            .map(|(position, chunk)| PositionedChunk { position, chunk })
            .collect(),
    })
}

pub async fn set_setting(hub: &Hub, p: SetDaemonSetting) -> Result<Done, RpcError> {
    if let Some(why) = invalid(&p.setting) {
        return Err(RpcError::new(ErrorCode::Rejected, why));
    }
    let log = hub.log.clone();
    blocking(move || log.append(Actor::Builder, DaemonEvent::SettingChanged(p.setting)))
        .await
        .map_err(|e| store_error(&e))?;
    // A higher cap per login lets agents waiting for a driver try again.
    hub.orch.reconsider();
    Ok(Done {})
}

/// A threshold of zero would stop a driver the moment it started or keep one forever.
fn invalid(setting: &DaemonSetting) -> Option<&'static str> {
    let zero = |d: &Duration| d.is_zero();
    match setting {
        DaemonSetting::RetireAfterAge(d)
        | DaemonSetting::StallTimeout(d)
        | DaemonSetting::DefaultWaitDeadline(d)
        | DaemonSetting::LastTurnLimit(d)
        | DaemonSetting::CoolOff(d)
            if zero(d) =>
        {
            Some("a duration must be above zero")
        }
        DaemonSetting::RetireAfterTasks(0)
        | DaemonSetting::TurnsWithoutProgress(0)
        | DaemonSetting::TerminalDriversPerLogin(0) => Some("a count must be above zero"),
        _ => None,
    }
}

/// Moves the fake clock, then fires what fell due and waits for the orchestrator to act on each,
/// as the job clocks do on the real clock, so the caller sees the reaction once this answers.
pub async fn set_clock(hub: &Hub, p: SetClock) -> Result<Done, RpcError> {
    let fake = hub.config.fake_clock.as_ref().ok_or_else(|| {
        RpcError::new(
            ErrorCode::WrongState,
            "the daemon runs on the real clock: start it with --fake-clock to move time",
        )
    })?;
    fake.set(p.at)
        .map_err(|e| RpcError::new(ErrorCode::Rejected, e.to_string()))?;
    let job_error = |e: x100_jobs::Error| RpcError::new(ErrorCode::Internal, e.to_string());
    for (id, jobs) in hub.schedulers() {
        let fired = blocking(move || {
            let mut fired = Vec::new();
            jobs.fire_due(|f| fired.push(f)).map(|()| fired)
        })
        .await
        .map_err(job_error)?;
        for f in fired {
            hub.orch.job_fired(id, f.job, f.at).await.map_err(refused)?;
        }
    }
    // The daemon's own jobs: cool-offs ending, which agents waiting for a driver may be waiting on.
    let jobs = hub.daemon_jobs();
    let fired = blocking(move || {
        let mut fired = Vec::new();
        jobs.fire_due(|f| fired.push(f)).map(|()| fired)
    })
    .await
    .map_err(job_error)?;
    for f in fired {
        hub.orch.daemon_job_fired(f.job).await;
    }
    Ok(Done {})
}

/// A machine's cap on live drivers. Lowering it stops no driver; raising it lets agents waiting
/// for room try again.
pub async fn set_cap(hub: &Hub, p: SetMachineCap) -> Result<Done, RpcError> {
    if hub
        .log
        .with_state(|d| d.machines().all(|m| m.id != p.machine))
    {
        return Err(RpcError::new(
            ErrorCode::NotFound,
            format!("no machine {}", p.machine),
        ));
    }
    let log = hub.log.clone();
    let event = DaemonEvent::MachineCapSet {
        machine: p.machine,
        cap: p.cap,
    };
    blocking(move || log.append(Actor::Builder, event))
        .await
        .map_err(|e| store_error(&e))?;
    hub.orch.reconsider();
    Ok(Done {})
}

/// Looks for harnesses again, records what changed, and lets the router use what was found.
pub async fn rescan(hub: &Hub, _: RescanHarnesses) -> Result<Rescanned, RpcError> {
    let (config, log, desk) = (hub.config.clone(), hub.log.clone(), hub.desk.clone());
    let found = blocking(move || crate::discover::discover(&config, &log, desk))
        .await
        .map_err(|e| RpcError::new(ErrorCode::Internal, e.to_string()))?;
    if found.harnesses.is_empty() {
        return Err(RpcError::new(
            ErrorCode::Unavailable,
            "no harness was found on this machine",
        ));
    }
    let mut harnesses: Vec<_> = found.harnesses.keys().copied().collect();
    harnesses.sort();
    hub.orch.set_harnesses(found.harnesses);
    Ok(Rescanned { harnesses })
}
