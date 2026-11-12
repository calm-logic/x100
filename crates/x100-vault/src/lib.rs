//! The encrypted credential store, references, the egress proxy, the wrapper for named
//! command-line tools, and redaction.
//!
//! Agents never hold credential values. An agent sees a reference such as
//! `ref_01JC6Q4ZB0Q1V7XK4N5W1D8M3E`. Values enter through [`Vault::add`] and [`Vault::replace`]
//! and no public function returns one: inside this crate only the proxy and the wrapper read
//! them, and only for a use the credential's scope allows.
//!
//! - [`Vault`]: values sealed at rest with XChaCha20-Poly1305 under a key from the OS keychain
//!   ([`OsKeychain`]) or, where there is none, from `vault.key`, a file only the builder's user
//!   can read.
//! - [`Proxy`]: the only route out of a sandbox. It ends TLS with the vault's [`Authority`],
//!   swaps references for values in headers, the query and the body when the request's host,
//!   project and driver allow it, redacts responses, and reports every request.
//! - [`Proxy::call`]: a request the daemon makes itself, such as the gate's call to a decision
//!   model, with one named credential's reference swapped in its headers.
//! - [`Wrapper`]: runs one named command with a value in its environment and redacts its output.
//! - [`Redactor`]: replaces known values, raw or encoded, with their references.
//!
//! # What the wrapper does not stop
//!
//! The wrapper narrows the routes to a credential. It is not a second sandbox.
//!
//! - It runs the named program as the daemon's user, outside the agent's sandbox, with direct
//!   network access. The credential's host list does not bind the program: it can send the value
//!   wherever it likes.
//! - It trusts the named program. General runners (shells, `make`, package script runners,
//!   interpreters, git) are refused whatever a credential's list says, and a program found in a
//!   directory the agent can write to is refused. But a named tool that reads files from the
//!   worktree can still be steered by them.
//! - A deploy tool that runs a build step from the project's config is refused while the agent's
//!   changes touch that config. The build step itself still runs the project's code, which the
//!   agent may have changed elsewhere.
//! - Output is redacted in the forms [`Redactor`] knows. A program that prints the value in
//!   another form (encrypted, compressed, reversed, a character at a time with escapes between)
//!   is not caught.
//!
//! # What the proxy does not do
//!
//! - It offers only HTTP/1.1 on the intercepted side, and does not forward `Upgrade` (WebSocket).
//! - Inside a `CONNECT` tunnel it expects TLS. Plain HTTP must come as absolute-form requests.
//! - A request body with a declared length over 4 MiB passes through unchanged, so a large
//!   upload keeps its length; references in it are not swapped.
//! - It asks the upstream for an uncompressed response so that it can redact it.
//!
//! # Limits of redaction
//!
//! Base64 leaves up to one character on each side of a value that also carries bits of the
//! bytes around it, so those few bits stay visible. The matcher's tables hold the values and are
//! not wiped on drop, unlike [`SecretValue`]. Buffers on the way out of the proxy (the request
//! with the value in it) are not wiped either.

mod encode;
mod keychain;
mod proxy;
mod redact;
mod refs;
mod scope;
mod seal;
mod secret;
mod store;
mod wrapper;

use std::path::PathBuf;

use x100_core::id::CredentialRef;

pub use keychain::{Keychain, NoKeychain, OsKeychain, Unavailable};
pub use proxy::{
    Admission, Admit, Authority, Called, DaemonCall, EgressOutcome, EgressReport, Proxy,
    ProxyConfig, Resolved, Route, UpstreamTrust, sandbox_env,
};
pub use redact::{RedactStream, Redactor};
pub use scope::Requester;
pub use seal::KeySource;
pub use secret::{MIN_LEN, SecretValue};
pub use store::Vault;
pub use wrapper::{WrapError, WrapOutput, WrapRequest, Wrapper};

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("the vault's key is missing: {0}")]
    KeyMissing(String),
    #[error("the vault's key does not open {0}: the key is wrong or the file was changed")]
    WrongKey(PathBuf),
    #[error("the vault's key is in the OS keychain, which cannot be reached: {0}")]
    KeychainUnavailable(String),
    #[error("{0} can be read by others (mode {1:o}); it must be 600")]
    KeyFileExposed(PathBuf, u32),
    #[error("{0}")]
    Corrupt(String),
    #[error("no credential {0}")]
    Unknown(CredentialRef),
    #[error("a credential {0} already exists")]
    Exists(CredentialRef),
    #[error("a value must be at least {0} bytes, or it cannot be told apart in output")]
    TooShort(usize),
    #[error("certificate: {0}")]
    Certificate(String),
}
