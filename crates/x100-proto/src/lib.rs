//! The client protocol: every request, response and notification as types, and the line format
//! they travel in. No I/O and no async runtime: the daemon and the clients bring their own socket.
//!
//! The wire is JSON-RPC 2.0, one JSON object per line. `PROTOCOL.md` in this crate lists every
//! method, and a test keeps it in step with [`METHODS`].

mod bytes;
mod error;
mod methods;
pub mod secret;
mod types;
mod wire;

pub use bytes::Bytes;
pub use error::{ErrorCode, RpcError};
pub use methods::{Call, METHODS, Method, MethodInfo, NOTICES, Notice, reserialize_result};
pub use secret::Secret;
pub use types::*;
pub use wire::{BadLine, Id, Message, Request, Response};

/// The version `hello` negotiates.
pub const PROTOCOL_VERSION: u32 = 1;
