//! One id type per kind of record, so an id of one kind cannot be passed where another is expected.
//!
//! Every id wraps a ULID. Generating one needs the time and randomness, which are I/O, so this
//! crate only builds ids from parts: the daemon supplies `Ulid::from_parts(clock.now().as_millis(),
//! random)`.
//!
//! On the wire an id is its kind's prefix and the ULID, for example
//! `agt_01JC6Q4ZB0Q1V7XK4N5W1D8M3E`. Parsing checks the prefix, so a task id sent where an agent id
//! belongs is refused at the boundary.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
pub use ulid::Ulid;

/// Why a string is not an id of the expected kind.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("expected an id starting with `{expected}_`, found `{found}`")]
    WrongKind {
        expected: &'static str,
        found: String,
    },
    #[error("`{0}` is not a valid ULID")]
    NotUlid(String),
}

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident => $prefix:literal;)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Ulid);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub const fn new(ulid: Ulid) -> Self {
                Self(ulid)
            }

            pub const fn ulid(self) -> Ulid {
                self.0
            }

            /// A small, readable id for tests and models, where uniqueness within one test is
            /// enough.
            pub const fn from_u128(n: u128) -> Self {
                Self(Ulid(n))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{}", $prefix, self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, IdError> {
                let rest = s
                    .strip_prefix($prefix)
                    .and_then(|r| r.strip_prefix('_'))
                    .ok_or_else(|| IdError::WrongKind { expected: $prefix, found: s.to_owned() })?;
                Ulid::from_string(rest).map(Self).map_err(|_| IdError::NotUlid(rest.to_owned()))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}

ids! {
    /// A project: one body of work with one CEO.
    ProjectId => "prj";
    /// A member of a project's team: the CEO, a lead or a worker.
    AgentId => "agt";
    /// One harness running one model at one effort level on one machine, for one agent.
    DriverId => "drv";
    /// An agent program x100 runs as drivers.
    HarnessId => "hns";
    /// A computer or VM that runs drivers.
    MachineId => "mch";
    /// Raised or delegated work.
    TaskId => "tsk";
    /// A message between agents, or between the builder and the CEO.
    MessageId => "msg";
    /// One thing delivered to an agent's inbox.
    InboxItemId => "inb";
    /// A scheduled job, in a project's log or the daemon's.
    JobId => "job";
    /// One thing an agent waits for.
    WaitId => "wai";
    /// Something an agent asked to do, which the gate classifies.
    ActionId => "act";
    /// A file the project holds outside the code.
    FileId => "fil";
    /// A reflection trial.
    TrialId => "trl";
    /// A single model turn with no tools and no agent, made by the gate's classifier or the lab's
    /// judge.
    OneOffCallId => "one";
    /// A credential, by reference. The value lives only in the vault.
    CredentialRef => "ref";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_round_trips_through_its_text_form() {
        let id = AgentId::new(Ulid::from_parts(1_700_000_000_000, 42));
        let text = id.to_string();
        assert!(text.starts_with("agt_"));
        assert_eq!(text.parse::<AgentId>(), Ok(id));
    }

    #[test]
    fn an_id_of_another_kind_is_refused() {
        let task = TaskId::from_u128(7).to_string();
        assert!(matches!(
            task.parse::<AgentId>(),
            Err(IdError::WrongKind {
                expected: "agt",
                ..
            })
        ));
    }

    #[test]
    fn a_bad_ulid_is_refused() {
        assert!(matches!(
            "agt_nope".parse::<AgentId>(),
            Err(IdError::NotUlid(_))
        ));
    }

    #[test]
    fn ids_serialize_as_prefixed_strings() {
        let id = CredentialRef::from_u128(1);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<CredentialRef>(&json).unwrap(), id);
        assert!(
            serde_json::from_str::<CredentialRef>(&format!("\"{}\"", FileId::from_u128(1)))
                .is_err()
        );
    }
}
