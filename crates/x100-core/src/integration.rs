//! Integrations: services outside x100 that the builder connects once and projects then use.
//! Homebox, the owner's self-hosted deploy platform, is the first, and GitHub the second, since
//! Homebox deploys only from a hosted repository.
//!
//! A connection is one of the daemon's settings, and a project's link to something in one is a
//! project setting, so both are events in their logs and fields of the folded state. A key is in
//! the vault and only its reference is here. Every call to a service is the daemon's own, so no
//! agent holds a credential.

use serde::{Deserialize, Serialize};

use crate::id::CredentialRef;

/// How a Homebox was connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeboxWay {
    /// A Homebox on this machine: its container minted an edit key for x100.
    Local,
    /// A host address and an API key the builder pasted.
    Address,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HomeboxConnection {
    pub way: HomeboxWay,
    /// Where its API is, as `scheme://host[:port]`, without `/api`. Its key's credential is
    /// scoped to this host (and port) alone, and to no project.
    pub address: String,
    pub credential: CredentialRef,
    /// The domain a deploy is public under, as `<project>.<primary domain>`, if the API said.
    pub primary_domain: Option<String>,
    /// The GitHub accounts its integrations cover, as far as the API said: the one to connect
    /// to x100.
    pub github_accounts: Vec<String>,
}

/// How GitHub was connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubWay {
    /// The `gh` command line, logged in on this machine. The daemon runs it and never reads or
    /// holds its token.
    LocalGh,
    /// GitHub's device flow for an OAuth app registered for x100: the token is in the vault.
    DeviceFlow { credential: CredentialRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GitHubConnection {
    pub way: GitHubWay,
    /// The account x100 creates repositories under.
    pub account: String,
}

/// A project's link to a Homebox project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HomeboxLink {
    /// The address of the Homebox it is on, as its connection names it.
    pub instance: String,
    /// The stable key: a Homebox project is one repository, `owner/name`.
    pub repo_full_name: String,
    /// The id and name as last seen. A renamed project changes address; the link finds it again
    /// by `repo_full_name`.
    pub project_id: String,
    pub project_name: String,
    /// The environment a deploy goes to, such as `production`.
    pub environment: String,
}

/// The environment a deploy goes to when none is named.
pub const DEFAULT_ENVIRONMENT: &str = "production";
