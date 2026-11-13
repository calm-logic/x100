//! Methods about integrations: Homebox and GitHub, connected once by the builder, and a project's
//! link to a Homebox project.

use serde::{Deserialize, Serialize};
use x100_core::id::ProjectId;
use x100_core::integration::{GitHubConnection, HomeboxConnection, HomeboxLink, HomeboxWay};

use crate::secret::{self, Secret};

/// Lists the integrations x100 supports, each with whether it is connected and the ways it can be
/// connected on this machine now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListIntegrations {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationList {
    pub homebox: HomeboxStatus,
    pub github: GitHubStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeboxStatus {
    pub connected: Option<HomeboxConnection>,
    /// The address of a Homebox found on this machine, which "use local" connects to. Found
    /// without a key: its container, its port, its health and its API's title.
    pub local: Option<String>,
    /// The way offered first: `local` when one is found here.
    pub default_way: HomeboxWay,
}

/// A way GitHub can be connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubMethod {
    /// The `gh` command line, logged in on this machine.
    LocalGh,
    /// GitHub's device flow for an OAuth app registered for x100.
    DeviceFlow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubStatus {
    pub connected: Option<GitHubConnection>,
    /// The ways that can be used here: `local_gh` when `gh` is installed and logged in,
    /// `device_flow` when the daemon is given an OAuth app's client id.
    pub ways: Vec<GitHubMethod>,
    /// The way offered first, `local_gh` when it can be used; none when neither can.
    pub default_way: Option<GitHubMethod>,
}

/// Connects Homebox, replacing one connected before.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "way", rename_all = "snake_case")]
pub enum ConnectHomebox {
    /// The Homebox on this machine: one fixed command in its container mints an edit key named
    /// x100, which goes straight to the vault. The builder confirms before this is sent.
    Local,
    /// A Homebox elsewhere: its address and an API key made there.
    Address {
        address: String,
        #[serde(with = "secret::wire")]
        key: Secret,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeboxConnected {
    pub connection: HomeboxConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectGitHub {
    pub way: GitHubMethod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubConnecting {
    /// Connected, as this account.
    Connected { account: String },
    /// The builder approves x100 at `verification_uri` with `user_code`; the connection appears
    /// in the daemon's settings once they have.
    Approve {
        user_code: String,
        verification_uri: String,
    },
}

/// Which integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integration {
    Homebox,
    GitHub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectIntegration {
    pub integration: Integration,
}

/// The connected Homebox's projects, for a link to be chosen from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListHomeboxProjects {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeboxProject {
    pub name: String,
    /// The stable key: a Homebox project is one repository, `owner/name`.
    pub repo_full_name: String,
    /// Whether Homebox deploys it. Only a managed one can be linked.
    pub managed: bool,
    pub environments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeboxProjects {
    pub projects: Vec<HomeboxProject>,
}

/// Links a project to a Homebox project, by its name or its repository's full name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkHomebox {
    pub project: ProjectId,
    pub homebox_project: String,
    /// `production` unless named.
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomeboxLinked {
    pub link: HomeboxLink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlinkHomebox {
    pub project: ProjectId,
}

/// The builder deploys a project's main branch through Homebox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployProject {
    pub project: ProjectId,
    /// The linked one unless named.
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDeployed {
    /// The public address, when the project has a public service.
    pub address: Option<String>,
    pub commit: String,
}
