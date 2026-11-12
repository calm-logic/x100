//! Methods about the daemon, its machines, harnesses, credentials and decision model.

use serde::{Deserialize, Serialize};
use x100_core::credential::CredentialScope;
use x100_core::decision::DecisionProvider;
use x100_core::id::{CredentialRef, HarnessId, MachineId};
use x100_core::settings::DaemonSetting;
use x100_core::time::Timestamp;

use crate::secret::{self, Secret};

/// The first request on a connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Who is connecting, for the daemon's log: `x100-app`, `x100`.
    pub client: String,
    /// The highest [`crate::PROTOCOL_VERSION`] the client speaks.
    pub protocol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    /// The version both sides will use.
    pub protocol: u32,
    /// The daemon's own version.
    pub daemon: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetMachineCap {
    pub machine: MachineId,
    pub cap: u32,
}

/// Looks for harnesses on this machine again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RescanHarnesses {}

/// Reads the daemon's state: machines, harnesses, credentials, cool-offs and settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GetDaemon {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rescanned {
    /// Every harness found, new ones included.
    pub harnesses: Vec<HarnessId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDaemonSetting {
    pub setting: DaemonSetting,
}

/// Moves a daemon's fake clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetClock {
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddCredential {
    pub name: String,
    #[serde(with = "secret::wire")]
    pub value: Secret,
    pub scope: CredentialScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialAdded {
    pub credential: CredentialRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceCredential {
    pub credential: CredentialRef,
    #[serde(with = "secret::wire")]
    pub value: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeCredential {
    pub credential: CredentialRef,
    pub scope: CredentialScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveCredential {
    pub credential: CredentialRef,
}

/// Connects a decision model to classify: its key goes into the vault, scoped to the provider's
/// host and to no project, and the daemon's setting names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectDecisionModel {
    pub provider: DecisionProvider,
    #[serde(with = "secret::wire")]
    pub key: Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionModelConnected {
    /// The credential that holds the key.
    pub credential: CredentialRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisconnectDecisionModel {}
