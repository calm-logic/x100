//! `integrations.*`, and the project methods that use them: connecting Homebox and GitHub, a
//! project's link to a Homebox project, and the builder's own deploy.
//!
//! Homebox connects as a decision model does: its key goes into the vault as a credential scoped
//! to that Homebox's host and port and to no project, so the proxy gives it to no driver and only
//! the daemon's own calls can use it; then the daemon's setting names it. The key is checked with
//! Homebox before the setting is made, and a connection made before is replaced and its key
//! removed. GitHub's "use local" way holds nothing: `gh` keeps its own token.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use x100_core::credential::{Credential, CredentialScope, Projects};
use x100_core::event::{DaemonEvent, ProjectEvent};
use x100_core::id::CredentialRef;
use x100_core::integration::{GitHubConnection, GitHubWay, HomeboxConnection, HomeboxWay};
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_core::settings::{DaemonSetting, DaemonSettings};
use x100_integrations::api::Endpoint;
use x100_integrations::{Integrations, ProjectAt, Record, github};
use x100_proto::{
    ConnectGitHub, ConnectHomebox, DeployProject, DisconnectIntegration, Done, ErrorCode,
    GitHubConnecting, GitHubMethod, GitHubStatus, HomeboxConnected, HomeboxLinked, HomeboxProject,
    HomeboxProjects, HomeboxStatus, Integration, IntegrationList, LinkHomebox, ProjectDeployed,
    RpcError, UnlinkHomebox,
};
use x100_store::{DaemonLog, ProjectStore};
use x100_vault::{SecretValue, Vault};

use super::credentials::vault_error;
use super::{blocking, project, store_error};
use crate::hub::Hub;

fn rejected(why: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::Rejected, why)
}

fn settings(hub: &Hub) -> DaemonSettings {
    hub.log.with_state(|d| d.settings().clone())
}

pub async fn list(hub: &Hub) -> IntegrationList {
    let s = settings(hub);
    let i = &hub.integrations;
    let (local, gh) = tokio::join!(i.find_local(), i.gh.logged_in());
    let mut ways = Vec::new();
    if gh {
        ways.push(GitHubMethod::LocalGh);
    }
    if i.github_client_id.is_some() {
        ways.push(GitHubMethod::DeviceFlow);
    }
    IntegrationList {
        homebox: HomeboxStatus {
            connected: s.homebox,
            default_way: if local.is_some() {
                HomeboxWay::Local
            } else {
                HomeboxWay::Address
            },
            local: local.map(|l| l.address),
        },
        github: GitHubStatus {
            connected: s.github,
            default_way: ways.first().copied(),
            ways,
        },
    }
}

pub async fn connect_homebox(hub: &Hub, p: ConnectHomebox) -> Result<HomeboxConnected, RpcError> {
    let i = &hub.integrations;
    let (way, at, key) = match p {
        ConnectHomebox::Local => {
            let found = i.find_local().await.ok_or_else(|| {
                rejected(
                    "no Homebox was found on this machine: its container, homebox-admin, is not \
                     running, or does not answer as Homebox. Give its address and a key instead.",
                )
            })?;
            let at = Endpoint::parse(&found.address).map_err(rejected)?;
            // The one command run in its container, once, after the builder confirmed.
            let key = i.local.mint().await.map_err(rejected)?;
            (HomeboxWay::Local, at, key)
        }
        ConnectHomebox::Address { address, key } => {
            let at = Endpoint::parse(&address).map_err(rejected)?;
            (
                HomeboxWay::Address,
                at,
                SecretValue::from(key.expose().trim()),
            )
        }
    };
    let authority = at.authority();
    let credential = Credential {
        reference: x100_orch::new_id(&*hub.clock, CredentialRef::new),
        name: format!("Homebox key for {authority}"),
        scope: CredentialScope {
            hosts: BTreeSet::from([authority.clone()]),
            commands: BTreeSet::new(),
            projects: Projects::Only(BTreeSet::new()),
        },
    };
    let reference = credential.reference;
    let vault = hub.vault.clone();
    let held = credential.clone();
    blocking(move || vault.add(held, key))
        .await
        .map_err(vault_error)?;
    let at = i.endpoint_at(at.with(reference));
    let checked = i.homebox(&at).me().await;
    let role = match checked {
        Ok(me) => me.role,
        Err(e) => {
            let _ = hub.vault.remove(reference);
            return Err(match e.status {
                Some(401 | 403) => rejected(format!("Homebox at {authority} refused the key")),
                _ => RpcError::new(
                    ErrorCode::Unavailable,
                    format!("Homebox at {authority} could not be asked: {}", e.why),
                ),
            });
        }
    };
    if role == "read" {
        let _ = hub.vault.remove(reference);
        return Err(rejected(
            "the key can only read: x100 deploys, which needs an edit key",
        ));
    }
    let (primary_domain, github_accounts) = i.learn(&at).await;
    let connection = HomeboxConnection {
        way,
        address: at.address(),
        credential: reference,
        primary_domain,
        github_accounts,
    };
    let (vault, log, set) = (hub.vault.clone(), hub.log.clone(), connection.clone());
    blocking(move || {
        let before = log.with_state(|d| d.settings().homebox.clone());
        let undo = |e: x100_store::Error| {
            let _ = vault.remove(reference);
            store_error(&e)
        };
        log.append(Actor::Builder, DaemonEvent::CredentialAdded(credential))
            .map_err(undo)?;
        let event = DaemonEvent::SettingChanged(DaemonSetting::Homebox(Some(set)));
        if let Err(e) = log.append(Actor::Builder, event) {
            forget(&vault, &log, reference);
            return Err(store_error(&e));
        }
        if let Some(old) = before {
            forget(&vault, &log, old.credential);
        }
        Ok(())
    })
    .await?;
    Ok(HomeboxConnected { connection })
}

pub async fn connect_github(hub: &Hub, p: ConnectGitHub) -> Result<GitHubConnecting, RpcError> {
    let i = &hub.integrations;
    match p.way {
        GitHubMethod::LocalGh => {
            if !i.gh.logged_in().await {
                return Err(rejected(
                    "gh is not installed here, or not logged in: run `gh auth login` in a \
                     terminal, then connect again",
                ));
            }
            let account = i.gh.account().await.map_err(rejected)?;
            let connection = GitHubConnection {
                way: GitHubWay::LocalGh,
                account: account.clone(),
            };
            set_github(hub, connection).await?;
            Ok(GitHubConnecting::Connected { account })
        }
        GitHubMethod::DeviceFlow => {
            let client_id = i.github_client_id.clone().ok_or_else(|| {
                RpcError::new(
                    ErrorCode::Unavailable,
                    "GitHub's sign-in needs an OAuth app registered for x100, and this daemon \
                     was given none: connect with the local gh instead",
                )
            })?;
            let code = github::device_start(&i.proxy, &client_id)
                .await
                .map_err(|why| RpcError::new(ErrorCode::Unavailable, why))?;
            let answer = GitHubConnecting::Approve {
                user_code: code.user_code.clone(),
                verification_uri: code.verification_uri.clone(),
            };
            tokio::spawn(device_flow(
                Arc::clone(&hub.integrations),
                hub.vault.clone(),
                hub.log.clone(),
                Arc::clone(&hub.clock),
                client_id,
                code,
            ));
            Ok(answer)
        }
    }
}

/// Polls GitHub until the builder approves x100, then keeps the token in the vault, scoped to
/// GitHub's hosts and to no project, and connects. Gives up when GitHub says the flow is over.
async fn device_flow(
    i: Arc<Integrations>,
    vault: Vault,
    log: DaemonLog,
    clock: Arc<dyn x100_core::time::Clock>,
    client_id: String,
    code: github::DeviceCode,
) {
    let mut every = std::time::Duration::from_secs(code.interval.max(1));
    let token = loop {
        tokio::time::sleep(every).await;
        match github::device_poll(&i.proxy, &client_id, &code.device_code).await {
            github::Polled::Waiting { slower } => {
                if slower {
                    every += std::time::Duration::from_secs(5);
                }
            }
            github::Polled::Token(token) => break token,
            github::Polled::Over(why) => {
                eprintln!("x100d: connecting GitHub through its sign-in ended: {why}");
                return;
            }
        }
    };
    let credential = Credential {
        reference: x100_orch::new_id(&*clock, CredentialRef::new),
        name: "GitHub token".into(),
        scope: CredentialScope {
            hosts: github::OAUTH_HOSTS
                .iter()
                .map(|h| (*h).to_owned())
                .collect(),
            commands: BTreeSet::new(),
            projects: Projects::Only(BTreeSet::new()),
        },
    };
    let reference = credential.reference;
    if let Err(e) = vault.add(credential.clone(), token) {
        eprintln!("x100d: GitHub's token could not be kept: {e}");
        return;
    }
    let account = match github::oauth_account(&i.proxy, reference).await {
        Ok(a) => a,
        Err(why) => {
            let _ = vault.remove(reference);
            eprintln!("x100d: GitHub did not say whose token it gave: {why}");
            return;
        }
    };
    let connection = GitHubConnection {
        way: GitHubWay::DeviceFlow {
            credential: reference,
        },
        account,
    };
    let done = blocking(move || {
        if let Err(e) = log.append(Actor::Builder, DaemonEvent::CredentialAdded(credential)) {
            let _ = vault.remove(reference);
            return Err(e);
        }
        set_github_in(&vault, &log, connection)
    })
    .await;
    if let Err(e) = done {
        eprintln!("x100d: GitHub's connection is not in the log: {e}");
    }
}

async fn set_github(hub: &Hub, connection: GitHubConnection) -> Result<(), RpcError> {
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || set_github_in(&vault, &log, connection))
        .await
        .map_err(|e| store_error(&e))
}

/// Makes `connection` GitHub's, and removes the token of the one before it, if it had one.
fn set_github_in(
    vault: &Vault,
    log: &DaemonLog,
    connection: GitHubConnection,
) -> Result<(), x100_store::Error> {
    let before = log.with_state(|d| d.settings().github.clone());
    let event = DaemonEvent::SettingChanged(DaemonSetting::GitHub(Some(connection)));
    log.append(Actor::Builder, event)?;
    if let Some(GitHubWay::DeviceFlow { credential }) = before.map(|b| b.way) {
        forget(vault, log, credential);
    }
    Ok(())
}

pub async fn disconnect(hub: &Hub, p: DisconnectIntegration) -> Result<Done, RpcError> {
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        let s = log.with_state(|d| d.settings().clone());
        let (event, credential) = match p.integration {
            Integration::Homebox => match s.homebox {
                None => return Ok(()),
                Some(c) => (DaemonSetting::Homebox(None), Some(c.credential)),
            },
            Integration::GitHub => match s.github {
                None => return Ok(()),
                Some(g) => (
                    DaemonSetting::GitHub(None),
                    match g.way {
                        GitHubWay::DeviceFlow { credential } => Some(credential),
                        GitHubWay::LocalGh => None,
                    },
                ),
            },
        };
        log.append(Actor::Builder, DaemonEvent::SettingChanged(event))
            .map_err(|e| store_error(&e))?;
        if let Some(credential) = credential {
            forget(&vault, &log, credential);
        }
        Ok(())
    })
    .await?;
    Ok(Done {})
}

/// Removes a key that no connection names any more. Best effort: a value the log still names is
/// reconciled away at the next start.
fn forget(vault: &Vault, log: &DaemonLog, credential: CredentialRef) {
    if let Err(e) = vault.remove(credential) {
        eprintln!("x100d: the old integration key {credential} could not be removed: {e}");
        return;
    }
    if let Err(e) = log.append(
        Actor::Builder,
        DaemonEvent::CredentialRemoved { credential },
    ) {
        eprintln!("x100d: the removal of {credential} is not in the log: {e}");
    }
}

/// The setting that ends when the builder removes the key it names from Credentials, so no
/// connection names a key that is gone.
pub(crate) fn without(
    settings: &DaemonSettings,
    credential: CredentialRef,
) -> Option<DaemonSetting> {
    if settings
        .homebox
        .as_ref()
        .is_some_and(|c| c.credential == credential)
    {
        return Some(DaemonSetting::Homebox(None));
    }
    match settings.github.as_ref().map(|g| g.way) {
        Some(GitHubWay::DeviceFlow { credential: c }) if c == credential => {
            Some(DaemonSetting::GitHub(None))
        }
        _ => None,
    }
}

pub(super) fn removed(log: &DaemonLog, credential: CredentialRef) -> Result<(), RpcError> {
    let Some(setting) = log.with_state(|d| without(d.settings(), credential)) else {
        return Ok(());
    };
    log.append(Actor::Builder, DaemonEvent::SettingChanged(setting))
        .map(drop)
        .map_err(|e| store_error(&e))
}

fn connected(hub: &Hub) -> Result<HomeboxConnection, RpcError> {
    settings(hub)
        .homebox
        .ok_or_else(|| RpcError::new(ErrorCode::WrongState, x100_integrations::NO_HOMEBOX))
}

pub async fn homebox_projects(hub: &Hub) -> Result<HomeboxProjects, RpcError> {
    let conn = connected(hub)?;
    let projects = hub
        .integrations
        .homebox_projects(&conn)
        .await
        .map_err(|why| RpcError::new(ErrorCode::Unavailable, why))?;
    Ok(HomeboxProjects {
        projects: projects
            .into_iter()
            .map(|p| HomeboxProject {
                name: p.name,
                repo_full_name: p.repo_full_name,
                managed: p.managed,
                environments: p.environments.into_iter().map(|e| e.name).collect(),
            })
            .collect(),
    })
}

pub async fn link(hub: &Hub, p: LinkHomebox) -> Result<HomeboxLinked, RpcError> {
    let store = project(hub, p.project)?;
    let conn = connected(hub)?;
    let projects = hub
        .integrations
        .homebox_projects(&conn)
        .await
        .map_err(|why| RpcError::new(ErrorCode::Unavailable, why))?;
    let link = Integrations::link_to(
        &conn,
        &projects,
        &p.homebox_project,
        p.environment.as_deref(),
    )
    .map_err(rejected)?;
    let change = SettingChange::Homebox(Some(link.clone()));
    append(&store, change).await?;
    Ok(HomeboxLinked { link })
}

pub async fn unlink(hub: &Hub, p: UnlinkHomebox) -> Result<Done, RpcError> {
    let store = project(hub, p.project)?;
    if store
        .log
        .with_state(|s| s.project().is_some_and(|p| p.homebox.is_some()))
    {
        append(&store, SettingChange::Homebox(None)).await?;
    }
    Ok(Done {})
}

async fn append(store: &ProjectStore, change: SettingChange) -> Result<(), RpcError> {
    let log = store.log.clone();
    blocking(move || log.append(Actor::Builder, ProjectEvent::SettingChanged(change)))
        .await
        .map(drop)
        .map_err(|e| store_error(&e))
}

/// Records each setting a step of the builder's deploy makes true, as the builder's.
struct Builder(ProjectStore);

#[async_trait]
impl Record for Builder {
    async fn record(&self, change: SettingChange) -> Result<(), String> {
        append(&self.0, change).await.map_err(|e| e.message)
    }
}

pub async fn deploy(hub: &Hub, p: DeployProject) -> Result<ProjectDeployed, RpcError> {
    let store = project(hub, p.project)?;
    let s = settings(hub);
    let wrong = |why: String| RpcError::new(ErrorCode::WrongState, why);
    Integrations::can_deploy(&s).map_err(wrong)?;
    let at = store
        .log
        .with_state(|state| {
            let p = state.project()?;
            Some(ProjectAt::new(
                p.code_dir.as_deref()?,
                p.homebox.clone(),
                p.repository.clone(),
            ))
        })
        .ok_or_else(|| wrong("the project has no code yet".into()))?;
    let done = hub
        .integrations
        .deploy(&s, &at, p.environment.as_deref(), &Builder(store))
        .await
        .map_err(rejected)?;
    Ok(ProjectDeployed {
        address: done.address,
        commit: done.commit,
    })
}
