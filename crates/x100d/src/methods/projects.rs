use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_core::event::{DaemonEvent, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, ProjectId};
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_orch::new_id;
use x100_proto::{
    ChangeProjectSetting, CreateProject, Done, ErrorCode, GetProject, ProjectCreated, ProjectList,
    ProjectSummary, RpcError,
};
use x100_store::ProjectStore;

use super::{blocking, project, store_error};
use crate::defaults;
use crate::hub::Hub;

pub fn list(hub: &Hub) -> ProjectList {
    let entries: Vec<_> = hub.log.with_state(|d| {
        d.projects()
            .map(|(id, e)| (id, e.name.clone(), e.path.clone()))
            .collect()
    });
    let projects = entries
        .into_iter()
        .map(|(id, name, path)| {
            // A project whose creation was cut short has no open log yet.
            let (live, agents) = hub
                .project(id)
                .map(|s| s.log.with_state(|p| (p.is_live(), p.agent_count())))
                .unwrap_or_default();
            ProjectSummary {
                project: id,
                name,
                path,
                live,
                agents: agents as u32,
            }
        })
        .collect();
    ProjectList { projects }
}

pub fn get(hub: &Hub, p: GetProject) -> Result<ProjectState, RpcError> {
    Ok(project(hub, p.project)?.log.state())
}

pub async fn create(hub: &Hub, p: CreateProject) -> Result<ProjectCreated, RpcError> {
    let path = PathBuf::from(&p.path);
    if !path.is_absolute() {
        return Err(RpcError::new(
            ErrorCode::Rejected,
            "the path must be absolute",
        ));
    }
    let name = p
        .name
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .ok_or_else(|| RpcError::new(ErrorCode::Rejected, "the project needs a name"))?;
    let scripted = hub.config.scripts_dir.is_some();
    let settings = p
        .settings
        .unwrap_or_else(|| hub.log.with_state(|d| defaults::settings(d, scripted)));
    if hub
        .log
        .with_state(|d| d.machines().all(|m| m.harnesses.is_empty()))
    {
        return Err(RpcError::new(
            ErrorCode::Unavailable,
            "no harness is available on this machine, so no agent could work",
        ));
    }
    let text = p.path.clone();
    // A creation that stopped half way left the directory on the list with an empty log: finish it.
    let earlier = hub
        .log
        .with_state(|d| d.projects().find(|(_, e)| e.path == text).map(|(id, _)| id));
    if let Some(id) = earlier
        && hub.project(id).is_some()
    {
        return Err(RpcError::new(
            ErrorCode::Rejected,
            format!("{} is a project already", p.path),
        ));
    }

    let clock = Arc::clone(&hub.clock);
    let store = {
        let (path, clock) = (path.clone(), Arc::clone(&clock));
        blocking(move || open_store(&path, clock))
            .await
            .map_err(|e| RpcError::new(ErrorCode::Rejected, e))?
    };
    let id = match earlier {
        Some(id) => id,
        None => {
            let id = new_id(&*clock, ProjectId::new);
            let log = hub.log.clone();
            let event = DaemonEvent::ProjectAdded {
                project: id,
                name: name.clone(),
                path: text,
            };
            blocking(move || log.append(Actor::Builder, event))
                .await
                .map_err(|e| store_error(&e))?;
            id
        }
    };
    let created = ProjectEvent::Created {
        name,
        ceo: new_id(&*clock, AgentId::new),
        charter: "Own this project from the builder's brief to a result the builder accepts."
            .into(),
        effort: defaults::ceo_effort(),
        settings,
    };
    let log = store.log.clone();
    blocking(move || log.append(Actor::Builder, created))
        .await
        .map_err(|e| store_error(&e))?;
    hub.open_project(id, store.clone());
    hub.start_project(id, store);
    Ok(ProjectCreated { project: id })
}

fn open_store(path: &Path, clock: Arc<dyn x100_core::time::Clock>) -> Result<ProjectStore, String> {
    let store = ProjectStore::open(path, clock).map_err(|e| format!("{}: {e}", path.display()))?;
    if store.log.last().is_some() {
        return Err(format!("{} already holds an x100 project", path.display()));
    }
    store
        .git
        .prepare()
        .map_err(|e| format!("{} cannot be a project: {e}", path.display()))?;
    Ok(store)
}

pub async fn change_setting(hub: &Hub, p: ChangeProjectSetting) -> Result<Done, RpcError> {
    let store = project(hub, p.project)?;
    if let SettingChange::Allow {
        what,
        allowed: true,
    } = &p.change
    {
        let known = hub.log.with_state(|d| {
            d.harness(what.harness)
                .is_some_and(|h| h.models.iter().any(|m| m.model == what.model))
        });
        if !known {
            return Err(RpcError::new(
                ErrorCode::Rejected,
                "no such harness and model on this machine",
            ));
        }
    }
    let change = p.change;
    blocking(move || {
        store
            .log
            .append(Actor::Builder, ProjectEvent::SettingChanged(change))
    })
    .await
    .map_err(|e| store_error(&e))?;
    Ok(Done {})
}
