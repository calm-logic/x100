//! A credential's value on its way in.
//!
//! [`Secret`] has no `Serialize` or `Deserialize`, so a response type that holds one does not
//! compile. The three request types that carry one say so with `#[serde(with = "wire")]`, which is
//! the only way it crosses the wire, and a reviewer can grep for it.

use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// For the vault, which is the only place a value is used.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// `#[serde(with = "wire")]` for a request field that carries a value in.
pub mod wire {
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Secret;

    pub fn serialize<S: Serializer>(secret: &Secret, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(secret.expose())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Secret, D::Error> {
        String::deserialize(d).map(Secret)
    }
}

/// A response type cannot hold a value, because `Secret` is not `Serialize`:
///
/// ```compile_fail
/// #[derive(serde::Serialize)]
/// struct Leaks {
///     value: x100_proto::Secret,
/// }
/// ```
///
/// Nor `Deserialize`:
///
/// ```compile_fail
/// #[derive(serde::Deserialize)]
/// struct Leaks {
///     value: x100_proto::Secret,
/// }
/// ```
///
/// A request says on purpose that a field carries one:
///
/// ```
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct Request {
///     #[serde(with = "x100_proto::secret::wire")]
///     value: x100_proto::Secret,
/// }
/// ```
#[cfg(doctest)]
struct Doctests;
