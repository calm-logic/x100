//! Integrations: Homebox and GitHub, which the builder connects once and projects then use.
//!
//! Every call to a service is the daemon's own. A Homebox key is in the vault and goes out only
//! through [`x100_vault::Proxy::call`], which swaps its reference for the value in the headers,
//! so nothing here handles a value except a freshly minted or approved one on its way into the
//! vault. GitHub's "use local" way runs `gh`, which keeps its own token.
//!
//! - [`api`]: a JSON API through the proxy.
//! - [`homebox`]: Homebox's API, its local container, and the deploy rule.
//! - [`github`]: `gh` behind [`github::GitHubCli`], and the device flow.
//! - [`Integrations`]: what the daemon and the CEO's tools call.

pub mod api;
pub mod github;
pub mod homebox;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use x100_core::integration::{
    DEFAULT_ENVIRONMENT, GitHubConnection, GitHubWay, HomeboxConnection, HomeboxLink,
};
use x100_core::project::SettingChange;
use x100_core::settings::DaemonSettings;
use x100_vault::Proxy;

use api::Endpoint;
use homebox::deploy::Watch;
pub use homebox::deploy::{Deployed, Timing};
use homebox::{Homebox, Project};

pub use github::GitHubCli;
pub use homebox::local::LocalHomebox;

/// What an agent or the builder is told when GitHub is not connected.
pub const NO_GITHUB: &str = "GitHub is not connected, so the project's repository stays on this \
     machine and Homebox, which deploys only from a hosted repository, cannot deploy it. The \
     builder can connect GitHub in Settings, under Integrations.";

pub const NO_HOMEBOX: &str = "Homebox is not connected. The builder can connect it in Settings, \
     under Integrations.";

/// What the OAuth way cannot do yet.
pub const OAUTH_NO_PUSH: &str = "GitHub is connected through its OAuth app, and pushing with \
     that token is not built yet: the value would have to leave the vault for git. Connect \
     GitHub with the local gh instead.";

pub struct Integrations {
    pub proxy: Proxy,
    pub local: Arc<dyn LocalHomebox>,
    pub gh: Arc<dyn GitHubCli>,
    pub timing: Timing,
    /// The client id of an OAuth app registered for x100 on GitHub, for the device flow. None
    /// exists yet, so by default GitHub connects only through `gh`.
    pub github_client_id: Option<String>,
    /// The code directories of projects being deployed now, by the CEO's call or the builder's:
    /// two at once would push and deploy twice.
    deploying: Mutex<HashSet<PathBuf>>,
}

/// Held while a project is being deployed.
struct Deploying<'a> {
    of: &'a Mutex<HashSet<PathBuf>>,
    dir: PathBuf,
}

impl Drop for Deploying<'_> {
    fn drop(&mut self) {
        self.of.lock().expect("deploying lock").remove(&self.dir);
    }
}

/// Receives a project setting the moment a step makes it true, so a deploy that fails later
/// still leaves the repository and the link on record.
#[async_trait]
pub trait Record: Send + Sync {
    async fn record(&self, change: SettingChange) -> Result<(), String>;
}

/// What a project is, as a deploy or a publish needs it.
#[derive(Debug, Clone)]
pub struct ProjectAt {
    pub code_dir: PathBuf,
    /// A new repository's name: the code directory's, which is already a slug.
    pub repo_name: String,
    pub homebox: Option<HomeboxLink>,
    pub repository: Option<String>,
}

impl ProjectAt {
    pub fn new(
        code_dir: &Path,
        homebox: Option<HomeboxLink>,
        repository: Option<String>,
    ) -> ProjectAt {
        ProjectAt {
            repo_name: code_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "project".into()),
            code_dir: code_dir.to_owned(),
            homebox,
            repository,
        }
    }
}

/// What a local Homebox showed, without a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundLocal {
    pub address: String,
}

impl Integrations {
    pub fn new(
        proxy: Proxy,
        local: Arc<dyn LocalHomebox>,
        gh: Arc<dyn GitHubCli>,
        timing: Timing,
        github_client_id: Option<String>,
    ) -> Integrations {
        Integrations {
            proxy,
            local,
            gh,
            timing,
            github_client_id,
            deploying: Mutex::new(HashSet::new()),
        }
    }

    /// Why a deploy cannot be done with what is connected, if it cannot: Homebox to deploy to,
    /// and GitHub through `gh` to push with.
    pub fn can_deploy(settings: &DaemonSettings) -> Result<(), String> {
        settings.homebox.as_ref().ok_or(NO_HOMEBOX)?;
        local_gh(settings).map(drop)
    }

    /// The connected Homebox's API, with its key's reference. A Homebox on loopback has its port
    /// opened for the daemon's own calls, and for nothing else.
    pub fn endpoint(&self, conn: &HomeboxConnection) -> Result<Endpoint, String> {
        let at = Endpoint::parse(&conn.address)?.with(conn.credential);
        Ok(self.endpoint_at(at))
    }

    /// `at`, with its port opened on the proxy for the daemon's own calls if it is on loopback.
    pub fn endpoint_at(&self, at: Endpoint) -> Endpoint {
        if let Some(port) = at.loopback_port() {
            self.proxy.open_loopback(port);
        }
        at
    }

    pub fn homebox<'a>(&'a self, at: &'a Endpoint) -> Homebox<'a> {
        Homebox {
            proxy: &self.proxy,
            at,
        }
    }

    /// A Homebox on this machine: the `homebox-admin` container's published port, which answers
    /// `/api/healthz` and serves Homebox's OpenAPI document. Only reads, and sends no key.
    pub async fn find_local(&self) -> Option<FoundLocal> {
        let port = self.local.port().await?;
        let at = Endpoint::parse(&format!("http://127.0.0.1:{port}")).ok()?;
        self.proxy.open_loopback(port);
        let healthy = api::get(&self.proxy, &at, "/api/healthz")
            .await
            .is_ok_and(|v| v.get("ok") == Some(&serde_json::Value::Bool(true)));
        let titled = healthy
            && api::get(&self.proxy, &at, "/api/openapi.json")
                .await
                .is_ok_and(|v| {
                    v.pointer("/info/title").and_then(|t| t.as_str()) == Some(homebox::TITLE)
                });
        if titled {
            Some(FoundLocal {
                address: at.address(),
            })
        } else {
            self.proxy.close_loopback(port);
            None
        }
    }

    /// What a key's Homebox says about itself, asked once at connect: the primary domain and the
    /// GitHub accounts its integrations cover. Neither is needed to deploy, so a failure leaves
    /// them unknown.
    pub async fn learn(&self, at: &Endpoint) -> (Option<String>, Vec<String>) {
        let hb = self.homebox(at);
        let domain = hb.primary_domain().await.ok().flatten();
        let accounts = hb
            .integrations()
            .await
            .map(|all| {
                all.into_iter()
                    .filter(|i| i.provider == "github")
                    .filter_map(|i| i.account_login)
                    .collect()
            })
            .unwrap_or_default();
        (domain, accounts)
    }

    /// Homebox's projects, for a link to be chosen from.
    pub async fn homebox_projects(&self, conn: &HomeboxConnection) -> Result<Vec<Project>, String> {
        let at = self.endpoint(conn)?;
        self.homebox(&at).projects().await.map_err(|e| e.why)
    }

    /// The link a project named by the builder or the CEO makes, with every choice when the
    /// name finds none or several.
    pub fn link_to(
        conn: &HomeboxConnection,
        projects: &[Project],
        wanted: &str,
        environment: Option<&str>,
    ) -> Result<HomeboxLink, String> {
        let p = match homebox::find(projects, wanted) {
            homebox::Found::One(p) => p,
            homebox::Found::Choose(choices) if choices.is_empty() => {
                return Err("Homebox has no projects yet".into());
            }
            homebox::Found::Choose(choices) => {
                return Err(format!(
                    "\"{wanted}\" is not one Homebox project. Choose one of: {}",
                    choices.join("; ")
                ));
            }
        };
        if !p.managed {
            return Err(format!(
                "Homebox does not manage {} yet, so it cannot deploy it: add it in Homebox first",
                p.repo_full_name
            ));
        }
        let environment = environment.unwrap_or(DEFAULT_ENVIRONMENT).to_owned();
        if !p.environments.is_empty() && !p.environments.iter().any(|e| e.name == environment) {
            let names: Vec<&str> = p.environments.iter().map(|e| e.name.as_str()).collect();
            return Err(format!(
                "{} has no environment {environment}; it has: {}",
                p.name,
                names.join(", ")
            ));
        }
        Ok(HomeboxLink {
            instance: Endpoint::parse(&conn.address)
                .map(|e| e.address())
                .unwrap_or_else(|_| conn.address.clone()),
            repo_full_name: p.repo_full_name,
            project_id: p.id,
            project_name: p.name,
            environment,
        })
    }

    /// Puts the project's main branch in a new private repository on GitHub, or pushes it again
    /// to the one it has. Returns the repository's full name.
    async fn publish(
        &self,
        settings: &DaemonSettings,
        project: &ProjectAt,
        record: &dyn Record,
    ) -> Result<String, String> {
        let github = local_gh(settings)?;
        let full = match &project.repository {
            Some(full) => full.clone(),
            None => {
                let full = format!("{}/{}", github.account, project.repo_name);
                self.gh.create_private(&full).await?;
                record
                    .record(SettingChange::Repository(full.clone()))
                    .await?;
                full
            }
        };
        self.gh.push(&project.code_dir, &full, "main").await?;
        Ok(full)
    }

    /// Deploys the project's main branch through Homebox, publishing it to GitHub, adding it to
    /// Homebox and linking it first if it is not linked yet.
    pub async fn deploy(
        &self,
        settings: &DaemonSettings,
        project: &ProjectAt,
        environment: Option<&str>,
        record: &dyn Record,
    ) -> Result<Deployed, String> {
        let conn = settings.homebox.as_ref().ok_or(NO_HOMEBOX)?;
        local_gh(settings)?;
        let _deploying = self.deploying(&project.code_dir)?;
        let at = self.endpoint(conn)?;
        let hb = self.homebox(&at);
        let commit = head(&project.code_dir).await?;
        let (link, watch) = match &project.homebox {
            Some(link) => {
                if Endpoint::parse(&link.instance).map(|e| e.address()).ok() != Some(at.address()) {
                    return Err(format!(
                        "the project is linked to the Homebox at {}, but the one connected is at \
                         {}: link it again",
                        link.instance,
                        at.address()
                    ));
                }
                let mut link = link.clone();
                if let Some(env) = environment {
                    link.environment = env.to_owned();
                }
                self.gh
                    .push(&project.code_dir, &link.repo_full_name, "main")
                    .await?;
                (link, Watch::Rule)
            }
            None => {
                let full = self.publish(settings, project, record).await?;
                let env = environment.unwrap_or(DEFAULT_ENVIRONMENT);
                let (p, queued) = add_repository(&hb, &full, env).await?;
                let link = HomeboxLink {
                    instance: at.address(),
                    repo_full_name: full,
                    project_id: p.id,
                    project_name: p.name,
                    environment: env.to_owned(),
                };
                record
                    .record(SettingChange::Homebox(Some(link.clone())))
                    .await?;
                (link, queued.map_or(Watch::Rule, Watch::Queued))
            }
        };
        homebox::deploy::deploy(&hb, &link, &commit, watch, &self.timing).await
    }
}

impl Integrations {
    fn deploying(&self, dir: &Path) -> Result<Deploying<'_>, String> {
        if !self
            .deploying
            .lock()
            .expect("deploying lock")
            .insert(dir.to_owned())
        {
            return Err("a deploy of this project is already under way".into());
        }
        Ok(Deploying {
            of: &self.deploying,
            dir: dir.to_owned(),
        })
    }
}

fn local_gh(settings: &DaemonSettings) -> Result<&GitHubConnection, String> {
    let github = settings.github.as_ref().ok_or(NO_GITHUB)?;
    match github.way {
        GitHubWay::LocalGh => Ok(github),
        GitHubWay::DeviceFlow { .. } => Err(OAUTH_NO_PUSH.into()),
    }
}

/// Makes a repository a managed project: Homebox looks again at what the integration covering its
/// owner can see, and then adds it. Adding queues a first deploy in every environment, which is
/// returned for `environment` so it is watched and not deployed a second time.
async fn add_repository(
    hb: &Homebox<'_>,
    full: &str,
    environment: &str,
) -> Result<(Project, Option<String>), String> {
    let owner = full.split('/').next().unwrap_or_default();
    let integrations = hb.integrations().await.map_err(|e| e.why)?;
    let integration = integrations
        .iter()
        .find(|i| i.covers(owner))
        .ok_or_else(|| {
            format!(
                "Homebox has no GitHub integration that covers {owner}: connect that account in \
             Homebox's Integrations, then deploy again"
            )
        })?;
    hb.sync(&integration.id).await.map_err(|e| e.why)?;
    let projects = hb.projects().await.map_err(|e| e.why)?;
    let found = projects
        .into_iter()
        .find(|p| p.repo_full_name.eq_ignore_ascii_case(full))
        .ok_or_else(|| format!("Homebox does not see {full}, even after looking again"))?;
    if found.managed {
        return Ok((found, None));
    }
    let added = hb.add(&found.id).await.map_err(|e| e.why)?;
    let queued = added
        .deployments
        .iter()
        .find(|d| d.environment.eq_ignore_ascii_case(environment))
        .map(|d| d.id.clone());
    if queued.is_none()
        && let Some(note) = added.note
    {
        return Err(format!(
            "Homebox added {full} but did not deploy it: {note}"
        ));
    }
    Ok((found, queued))
}

/// The commit `main` points at in the project's repository.
async fn head(code_dir: &Path) -> Result<String, String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(code_dir)
        .args(["rev-parse", "--verify", "refs/heads/main^{commit}"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("git could not be run: {e}"))?;
    if !out.status.success() {
        return Err("the project has no main branch to deploy".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
