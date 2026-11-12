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
    ProjectSummary, RenameProject, RpcError,
};
use x100_store::{Git, ProjectStore, project_dir};

use super::{blocking, project, refused, store_error};
use crate::defaults;
use crate::hub::Hub;

pub fn list(hub: &Hub) -> ProjectList {
    let ids: Vec<ProjectId> = hub
        .log
        .with_state(|d| d.projects().map(|(id, _)| id).collect());
    let projects = ids
        .into_iter()
        // A project whose creation was cut short has no open log.
        .filter_map(|id| {
            let store = hub.project(id)?;
            Some(store.log.with_state(|p| {
                let project = p.project();
                ProjectSummary {
                    project: id,
                    name: project.and_then(|p| p.name()).map(str::to_owned),
                    dir: project
                        .and_then(|p| p.code_dir.as_ref())
                        .map(|d| d.display().to_string()),
                    live: p.is_live(),
                    agents: p.agent_count() as u32,
                }
            }))
        })
        .collect();
    ProjectList { projects }
}

pub fn get(hub: &Hub, p: GetProject) -> Result<ProjectState, RpcError> {
    Ok(project(hub, p.project)?.log.state())
}

/// A new project: its store in the daemon's data directory under its id, and its CEO. On an
/// existing folder, that folder is its code directory from the start.
pub async fn create(hub: &Hub, p: CreateProject) -> Result<ProjectCreated, RpcError> {
    let rejected = |m: String| RpcError::new(ErrorCode::Rejected, m);
    let code_dir = match &p.dir {
        None => None,
        Some(dir) => Some(existing_folder(hub, Path::new(dir)).map_err(rejected)?),
    };
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

    let clock = Arc::clone(&hub.clock);
    let id = new_id(&*clock, ProjectId::new);
    let store = {
        let (dir, clock) = (project_dir(&hub.config.data_dir, id), Arc::clone(&clock));
        let code = code_dir.clone();
        blocking(move || open_store(&dir, code.as_deref(), clock))
            .await
            .map_err(rejected)?
    };
    // A creation cut short after this leaves an empty log, which the daemon skips on start.
    let log = hub.log.clone();
    blocking(move || log.append(Actor::Builder, DaemonEvent::ProjectAdded { project: id }))
        .await
        .map_err(|e| store_error(&e))?;
    let mut events = vec![(
        Actor::Builder,
        ProjectEvent::Created {
            ceo: new_id(&*clock, AgentId::new),
            charter: "Own this project, from what the builder asks for to a result the builder \
                      accepts."
                .into(),
            effort: defaults::ceo_effort(),
            settings,
        },
    )];
    if let Some(dir) = code_dir {
        events.push((Actor::Builder, ProjectEvent::CodeDirSet { dir }));
    }
    let log = store.log.clone();
    blocking(move || log.append_all(events))
        .await
        .map_err(|e| store_error(&e))?;
    hub.open_project(id, store.clone());
    hub.start_project(id, store);
    Ok(ProjectCreated { project: id })
}

/// The folder a project starts on: absolute, there, and no other project's.
fn existing_folder(hub: &Hub, dir: &Path) -> Result<PathBuf, String> {
    if !dir.is_absolute() {
        return Err("the folder must be an absolute path".into());
    }
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let taken = list(hub)
        .projects
        .iter()
        .any(|p| p.dir.as_deref() == Some(&*dir.to_string_lossy()));
    if taken {
        return Err(format!("{} is a project already", dir.display()));
    }
    Ok(dir)
}

fn open_store(
    dir: &Path,
    code: Option<&Path>,
    clock: Arc<dyn x100_core::time::Clock>,
) -> Result<ProjectStore, String> {
    if let Some(code) = code {
        Git::new(code)
            .prepare()
            .map_err(|e| format!("{} cannot be a project: {e}", code.display()))?;
    }
    ProjectStore::open(dir, clock).map_err(|e| format!("{}: {e}", dir.display()))
}

pub async fn rename(hub: &Hub, p: RenameProject) -> Result<Done, RpcError> {
    hub.orch.rename(p.project, p.name).await.map_err(refused)?;
    Ok(Done {})
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
