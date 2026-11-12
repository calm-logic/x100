use serde::{Deserialize, Serialize};

use crate::credential::{Credential, CredentialScope};
use crate::harness::{CoolOff, Harness, HarnessModel, Machine};
use crate::id::{CredentialRef, JobId, MachineId, ProjectId};
use crate::job::{DaemonJob, Job, JobEvent};

/// An event in the daemon's log: what belongs to no project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonEvent {
    MachineAdded(Machine),
    MachineCapSet {
        machine: MachineId,
        cap: u32,
    },
    /// A harness was found on this machine, with its models, effort levels and prices.
    HarnessAdded(Harness),
    /// What a harness supports or offers changed. The whole record replaces the old one.
    HarnessUpdated(Harness),
    /// A harness and model failed in a way that looks like the provider's, and are skipped for
    /// every project until the cool-off ends. `ends` is the job that ends it.
    CoolOffStarted {
        cool_off: CoolOff,
        ends: JobId,
    },
    CoolOffEnded(HarnessModel),
    /// The value went to the vault. It is never in the log.
    CredentialAdded(Credential),
    /// A new value replaced the old one in the vault.
    CredentialReplaced {
        credential: CredentialRef,
    },
    CredentialScopeSet {
        credential: CredentialRef,
        scope: CredentialScope,
    },
    CredentialRemoved {
        credential: CredentialRef,
    },
    /// A project directory joined the daemon's list. The project's own log holds the rest.
    ProjectAdded {
        project: ProjectId,
        name: String,
        path: String,
    },
    JobScheduled(Job<DaemonJob>),
    Job {
        job: JobId,
        event: JobEvent,
    },
}
