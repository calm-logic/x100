use std::time::Duration;

use x100_core::event::DaemonEvent;
use x100_core::party::Actor;
use x100_core::settings::DaemonSetting;
use x100_proto::{
    Done, ErrorCode, LogId, LogPage, Page, PositionedEntry, ReadLog, ReadTranscript, RpcError,
    SetClock, SetDaemonSetting, TranscriptPage,
};

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

pub async fn set_setting(hub: &Hub, p: SetDaemonSetting) -> Result<Done, RpcError> {
    if let Some(why) = invalid(&p.setting) {
        return Err(RpcError::new(ErrorCode::Rejected, why));
    }
    let log = hub.log.clone();
    blocking(move || log.append(Actor::Builder, DaemonEvent::SettingChanged(p.setting)))
        .await
        .map_err(|e| store_error(&e))?;
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
        DaemonSetting::RetireAfterTasks(0) | DaemonSetting::TurnsWithoutProgress(0) => {
            Some("a count must be above zero")
        }
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
    // The daemon's own jobs have nothing to act on yet, as on the real clock.
    if let Some(jobs) = hub.daemon_jobs() {
        blocking(move || jobs.fire_due(|_| {}))
            .await
            .map_err(job_error)?;
    }
    Ok(Done {})
}
