//! Homebox's admin API, as far as x100 uses it. Its OpenAPI document has no response schemas, so
//! each answer is typed here by hand from Homebox's route handlers, with only the fields x100
//! reads; an id is a number there and a string here.
//!
//! Every call is under `/api`, with an `hbk_` key as `Authorization: Bearer`. An edit key can do
//! all of these.

pub mod deploy;
pub mod local;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use x100_vault::Proxy;

use crate::api::{self, ApiError, Endpoint};

/// The title of Homebox's OpenAPI document, by which a local server is told from anything else
/// on the same port.
pub const TITLE: &str = "Homebox Admin API";

/// An id, as a number or a string, kept as a string.
fn id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    match Value::deserialize(d)? {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(serde::de::Error::custom(format!("an id, not {other}"))),
    }
}

fn some_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        Value::Number(n) => Ok(Some(n.to_string())),
        other => Err(serde::de::Error::custom(format!("an id, not {other}"))),
    }
}

/// A project: one repository. `environments` is filled only when it is managed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
    pub repo_full_name: String,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub auto_deploy: bool,
    #[serde(default)]
    pub environments: Vec<Environment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
    /// The environment's latest deployment that was not canceled.
    #[serde(default)]
    pub deployment: Option<DeploymentSummary>,
    /// Those of the latest deployment.
    #[serde(default)]
    pub instances: Vec<Instance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentSummary {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub commit_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    #[serde(default)]
    pub service_name: Option<String>,
    /// `https://<host>` for a public service; none for an internal one.
    #[serde(default)]
    pub url: Option<String>,
    /// `running` once Homebox's probe got an answer; `unreachable` when it got none, as for a
    /// container that keeps crashing; also `unknown`, `provisioning`, `remote` and `stopped`.
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub status: String,
    #[serde(default)]
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub log_tail: Option<String>,
}

impl Deployment {
    /// Still on its way: queued, building, or waiting for checks or a promotion.
    pub fn in_progress(&self) -> bool {
        matches!(
            self.status.as_str(),
            "queued"
                | "cloning"
                | "dissecting"
                | "building"
                | "starting"
                | "pending_checks"
                | "pending_promotion"
                | "pending_e2e"
        )
    }
}

/// A source-code integration of Homebox's, such as a GitHub account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integration {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub provider: String,
    #[serde(default)]
    pub account_login: Option<String>,
    #[serde(default)]
    pub orgs: Vec<String>,
    #[serde(default)]
    pub status: Option<String>,
}

impl Integration {
    /// Whether it can see repositories of `owner`: its own account or one of its organisations.
    pub fn covers(&self, owner: &str) -> bool {
        self.provider == "github"
            && (self
                .account_login
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(owner))
                || self.orgs.iter().any(|o| o.eq_ignore_ascii_case(owner)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Domain {
    name: String,
    #[serde(default)]
    is_primary: bool,
}

/// What adding a repository did: the first deploy it queued in each environment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Added {
    #[serde(default)]
    pub deployments: Vec<QueuedDeployment>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueuedDeployment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub environment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Queued {
    #[serde(deserialize_with = "id")]
    id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Me {
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Listed {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub status: String,
    #[serde(default, deserialize_with = "some_id")]
    pub commit_sha: Option<String>,
}

fn read<T: for<'de> Deserialize<'de>>(value: Value, what: &str) -> Result<T, ApiError> {
    serde_json::from_value(value).map_err(|e| ApiError {
        status: None,
        why: format!("Homebox's {what} is not in the shape x100 knows: {e}"),
    })
}

/// One Homebox, reached at `at` with its key's credential.
pub struct Homebox<'a> {
    pub proxy: &'a Proxy,
    pub at: &'a Endpoint,
}

impl Homebox<'_> {
    /// Who the key is and its role: the check a pasted key passes before it is kept.
    pub async fn me(&self) -> Result<Me, ApiError> {
        read(
            api::get(self.proxy, self.at, "/api/auth/me").await?,
            "answer about the key",
        )
    }

    pub async fn projects(&self) -> Result<Vec<Project>, ApiError> {
        read(
            api::get(self.proxy, self.at, "/api/projects").await?,
            "project list",
        )
    }

    pub async fn project(&self, id: &str) -> Result<Project, ApiError> {
        let path = format!("/api/projects/{id}");
        read(api::get(self.proxy, self.at, &path).await?, "project")
    }

    pub async fn integrations(&self) -> Result<Vec<Integration>, ApiError> {
        read(
            api::get(self.proxy, self.at, "/api/integrations").await?,
            "integration list",
        )
    }

    /// The domain a deploy is public under, if one is marked primary.
    pub async fn primary_domain(&self) -> Result<Option<String>, ApiError> {
        let domains: Vec<Domain> = read(
            api::get(self.proxy, self.at, "/api/domains").await?,
            "domain list",
        )?;
        Ok(domains.into_iter().find(|d| d.is_primary).map(|d| d.name))
    }

    /// Makes Homebox look again at the repositories an integration can see, so a new one becomes
    /// a project it can add.
    pub async fn sync(&self, integration: &str) -> Result<(), ApiError> {
        let path = format!("/api/integrations/{integration}/sync");
        api::post(self.proxy, self.at, &path, None).await.map(drop)
    }

    /// Makes a project managed. Homebox makes its environments and queues a first deploy in each.
    pub async fn add(&self, project: &str) -> Result<Added, ApiError> {
        let path = format!("/api/projects/{project}/add");
        read(
            api::post(self.proxy, self.at, &path, Some(&json!({}))).await?,
            "answer to adding a project",
        )
    }

    /// Deploys the head of the environment's branch; no commit can be named. Returns the
    /// deployment's id.
    pub async fn deploy(&self, project: &str, environment: &str) -> Result<String, ApiError> {
        let path = format!("/api/projects/{project}/environments/{environment}/deploy");
        let queued: Queued = read(
            api::post(self.proxy, self.at, &path, None).await?,
            "answer to a deploy",
        )?;
        Ok(queued.id)
    }

    pub async fn deployment(&self, project: &str, id: &str) -> Result<Deployment, ApiError> {
        let path = format!("/api/projects/{project}/deployments/{id}");
        read(api::get(self.proxy, self.at, &path).await?, "deployment")
    }

    /// The environment's newest deployments, newest first.
    pub async fn deployments(
        &self,
        project: &str,
        environment: &str,
    ) -> Result<Vec<Listed>, ApiError> {
        let path = format!("/api/projects/{project}/environments/{environment}/deployments");
        read(
            api::get(self.proxy, self.at, &path).await?,
            "deployment list",
        )
    }
}

/// What a name the CEO or the builder gave finds among Homebox's projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    One(Project),
    /// Several fit, or none did: the names to choose from.
    Choose(Vec<String>),
}

/// Finds a project by its repository's full name, `owner/name`, or by its name, ignoring case;
/// an exact repository name wins over a name.
pub fn find(projects: &[Project], wanted: &str) -> Found {
    let wanted = wanted.trim();
    if let Some(p) = projects
        .iter()
        .find(|p| p.repo_full_name.eq_ignore_ascii_case(wanted))
    {
        return Found::One(p.clone());
    }
    let named: Vec<&Project> = projects
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(wanted))
        .collect();
    match named.as_slice() {
        [one] => Found::One((*one).clone()),
        [] => Found::Choose(projects.iter().map(described).collect()),
        several => Found::Choose(several.iter().map(|p| described(p)).collect()),
    }
}

/// A project as a choice: its name and repository, and whether Homebox manages it.
pub fn described(p: &Project) -> String {
    let unmanaged = if p.managed { "" } else { ", not managed" };
    format!("{} ({}{unmanaged})", p.name, p.repo_full_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: u64, name: &str, repo: &str) -> Project {
        read(
            json!({"id": id, "name": name, "repo_full_name": repo, "managed": true,
                   "auto_deploy": true, "environments": []}),
            "project",
        )
        .unwrap()
    }

    #[test]
    fn a_project_is_found_by_repository_or_name_and_otherwise_the_choices_are_given() {
        let all = [
            project(1, "foobar", "me/foobar"),
            project(2, "shop", "me/shop"),
            project(3, "shop", "team/shop"),
        ];
        assert_eq!(find(&all, "FooBar"), Found::One(all[0].clone()));
        assert_eq!(find(&all, "team/shop"), Found::One(all[2].clone()));
        assert_eq!(
            find(&all, "shop"),
            Found::Choose(vec!["shop (me/shop)".into(), "shop (team/shop)".into()])
        );
        let Found::Choose(all_three) = find(&all, "nothing") else {
            panic!("no match")
        };
        assert_eq!(all_three.len(), 3);
        assert_eq!(all[0].id, "1", "a numeric id is kept as a string");
    }
}
