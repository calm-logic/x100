//! Credentials by reference. No type in this crate can hold a credential's value: values go from a
//! client to the vault once and never come out, so no event can carry one.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::id::{AgentId, CredentialRef, DriverId, ProjectId};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Credential {
    pub reference: CredentialRef,
    pub name: String,
    pub scope: CredentialScope,
}

/// Where a credential may go.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialScope {
    /// Hosts the proxy may send it to.
    pub hosts: BTreeSet<String>,
    /// Named commands the wrapper may give it to.
    pub commands: BTreeSet<String>,
    pub projects: Projects,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Projects {
    All,
    Only(BTreeSet<ProjectId>),
}

/// One use of a credential, or one refused attempt. Every use is an event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialUse {
    pub credential: CredentialRef,
    pub agent: AgentId,
    pub driver: DriverId,
    pub via: Via,
    pub outcome: UseOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The egress proxy put it in a request to this host.
    Proxy { host: String },
    /// The wrapper gave it to this named command.
    Wrapper { command: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UseOutcome {
    Injected,
    Refused(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    HostNotAllowed,
    CommandNotAllowed,
    ProjectNotAllowed,
    /// The agent's changes touch the config a named tool would run a build step from.
    ConfigTouched,
    /// Lab drivers have no route to credentials.
    LabDriver,
}
