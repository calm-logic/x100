//! The protocol's methods that need no connection: one function each, over the shared [`Hub`].

mod agents;
mod chat;
mod credentials;
mod daemon;
mod decision;
mod files;
mod integrations;
mod projects;

pub(crate) use daemon::terminal_of;
pub(crate) use integrations::without as integration_without;

use serde::Serialize;
use serde_json::Value;
use x100_core::id::ProjectId;
use x100_orch::Refused;
use x100_proto::{Call, ErrorCode, RpcError};
use x100_store::ProjectStore;

use crate::hub::Hub;
use crate::unavailable;

pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("a blocking task panicked")
}

pub fn store_error(e: &x100_store::Error) -> RpcError {
    match e {
        x100_store::Error::Rejected(f) => RpcError::new(ErrorCode::Rejected, f.to_string()),
        other => RpcError::new(ErrorCode::Internal, other.to_string()),
    }
}

pub(crate) fn refused(e: Refused) -> RpcError {
    match e {
        Refused::NotFound(m) => RpcError::new(ErrorCode::NotFound, m),
        Refused::WrongState(m) => RpcError::new(ErrorCode::WrongState, m),
        Refused::Rejected(m) => RpcError::new(ErrorCode::Rejected, m),
        Refused::Store(e) => store_error(&e),
    }
}

pub(crate) fn project(hub: &Hub, id: ProjectId) -> Result<ProjectStore, RpcError> {
    hub.project(id)
        .ok_or_else(|| RpcError::new(ErrorCode::NotFound, format!("no project {id}")))
}

fn out<T: Serialize>(result: Result<T, RpcError>) -> Result<Value, RpcError> {
    result.map(|v| serde_json::to_value(v).expect("results serialise"))
}

/// Answers a request that is not about the connection (hello and the subscriptions are).
pub async fn call(hub: &Hub, call: Call) -> Result<Value, RpcError> {
    if let Some(error) = unavailable::not_yet(&call) {
        return Err(error);
    }
    match call {
        Call::ReadLog(p) => out(daemon::read_log(hub, p).await),
        Call::ReadTranscript(p) => out(daemon::read_transcript(hub, p).await),
        Call::ReadTerminal(p) => out(daemon::read_terminal(hub, p).await),
        Call::GetDaemon(_) => out(Ok(hub.log.state())),
        Call::SetDaemonSetting(p) => out(daemon::set_setting(hub, p).await),
        Call::SetClock(p) => out(daemon::set_clock(hub, p).await),
        Call::ListProjects(_) => out(Ok(projects::list(hub))),
        Call::GetProject(p) => out(projects::get(hub, p)),
        Call::CreateProject(p) => out(projects::create(hub, p).await),
        Call::ChangeProjectSetting(p) => out(projects::change_setting(hub, p).await),
        Call::RenameProject(p) => out(projects::rename(hub, p).await),
        Call::SendChat(p) => out(chat::send(hub, p).await),
        Call::RaiseTask(p) => out(chat::raise(hub, p).await),
        Call::UploadFile(p) => out(files::upload(hub, p).await),
        Call::ReadFile(p) => out(files::read(hub, p).await),
        Call::RemoveFile(p) => out(files::remove(hub, p).await),
        Call::CancelJob(p) => out(agents::cancel_job(hub, p).await),
        Call::DecideAction(p) => out(agents::decide(hub, p).await),
        Call::ReadInbox(p) => out(agents::inbox(hub, p)),
        Call::ReadWaits(p) => out(agents::waits(hub, p)),
        Call::ReadStartPack(p) => out(agents::start_pack(hub, p)),
        Call::StopDriver(p) => out(agents::stop_driver(hub, p).await),
        Call::ResizeTerminal(p) => out(agents::resize_terminal(hub, p).await),
        Call::SetLeadBudget(p) => out(agents::set_budget(hub, p).await),
        Call::SetEffortRange(p) => out(agents::set_effort(hub, p).await),
        Call::SetAppearance(p) => out(agents::set_appearance(hub, p).await),
        Call::SetMachineCap(p) => out(daemon::set_cap(hub, p).await),
        Call::RescanHarnesses(p) => out(daemon::rescan(hub, p).await),
        Call::AddCredential(p) => out(credentials::add(hub, p).await),
        Call::ReplaceCredential(p) => out(credentials::replace(hub, p).await),
        Call::ScopeCredential(p) => out(credentials::set_scope(hub, p).await),
        Call::RemoveCredential(p) => out(credentials::remove(hub, p).await),
        Call::ConnectDecisionModel(p) => out(decision::connect(hub, p).await),
        Call::DisconnectDecisionModel(p) => out(decision::disconnect(hub, p).await),
        Call::ListIntegrations(_) => out(Ok(integrations::list(hub).await)),
        Call::ConnectHomebox(p) => out(integrations::connect_homebox(hub, p).await),
        Call::ConnectGitHub(p) => out(integrations::connect_github(hub, p).await),
        Call::DisconnectIntegration(p) => out(integrations::disconnect(hub, p).await),
        Call::ListHomeboxProjects(_) => out(integrations::homebox_projects(hub).await),
        Call::LinkHomebox(p) => out(integrations::link(hub, p).await),
        Call::UnlinkHomebox(p) => out(integrations::unlink(hub, p).await),
        Call::DeployProject(p) => out(integrations::deploy(hub, p).await),
        Call::Hello(_)
        | Call::Subscribe(_)
        | Call::Unsubscribe(_)
        | Call::SubscribeTranscript(_)
        | Call::SubscribeTerminal(_) => {
            unreachable!("the connection answers these")
        }
        // Every other method is in the table of what arrives later, which answered above.
        other => Err(RpcError::new(
            ErrorCode::Internal,
            format!("{} has no handler", other.method()),
        )),
    }
}
