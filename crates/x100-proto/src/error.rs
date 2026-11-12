//! Typed errors. On the wire an error is `{"code": -32001, "message": "...", "data": ...}`, and the
//! code is one of [`ErrorCode`]. The first five are JSON-RPC's own.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Why a request failed, as a number clients can switch on. Each method lists the codes it can
/// return in `PROTOCOL.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The line was not JSON.
    ParseError,
    /// The JSON was not a JSON-RPC 2.0 message this protocol uses.
    InvalidRequest,
    MethodNotFound,
    /// The params do not fit the method.
    InvalidParams,
    /// A fault in the daemon, not in the request.
    Internal,
    /// The project, agent, task or other record the request names does not exist.
    NotFound,
    /// The record exists but is in a state that does not allow this: deciding an action that is not
    /// held, cancelling a job that has fired.
    WrongState,
    /// The request is well formed and the record is fine, but it breaks a rule: a lead's budgets
    /// would pass the project's total, a directory is not usable as a project. The message says
    /// which.
    Rejected,
    TooLarge,
    /// The client speaks a protocol version the daemon does not.
    UnsupportedProtocol,
    UnknownSubscription,
    /// The daemon cannot do it now: it is shutting down, or nothing on this machine can serve the
    /// request.
    Unavailable,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 12] = [
        ErrorCode::ParseError,
        ErrorCode::InvalidRequest,
        ErrorCode::MethodNotFound,
        ErrorCode::InvalidParams,
        ErrorCode::Internal,
        ErrorCode::NotFound,
        ErrorCode::WrongState,
        ErrorCode::Rejected,
        ErrorCode::TooLarge,
        ErrorCode::UnsupportedProtocol,
        ErrorCode::UnknownSubscription,
        ErrorCode::Unavailable,
    ];

    pub const fn number(self) -> i64 {
        match self {
            ErrorCode::ParseError => -32700,
            ErrorCode::InvalidRequest => -32600,
            ErrorCode::MethodNotFound => -32601,
            ErrorCode::InvalidParams => -32602,
            ErrorCode::Internal => -32603,
            ErrorCode::NotFound => -32001,
            ErrorCode::WrongState => -32002,
            ErrorCode::Rejected => -32003,
            ErrorCode::TooLarge => -32004,
            ErrorCode::UnsupportedProtocol => -32005,
            ErrorCode::UnknownSubscription => -32006,
            ErrorCode::Unavailable => -32007,
        }
    }

    /// The name used in `PROTOCOL.md`.
    pub const fn name(self) -> &'static str {
        match self {
            ErrorCode::ParseError => "parse_error",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::MethodNotFound => "method_not_found",
            ErrorCode::InvalidParams => "invalid_params",
            ErrorCode::Internal => "internal",
            ErrorCode::NotFound => "not_found",
            ErrorCode::WrongState => "wrong_state",
            ErrorCode::Rejected => "rejected",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::UnsupportedProtocol => "unsupported_protocol",
            ErrorCode::UnknownSubscription => "unknown_subscription",
            ErrorCode::Unavailable => "unavailable",
        }
    }

    pub fn from_number(n: i64) -> Option<ErrorCode> {
        ErrorCode::ALL.into_iter().find(|c| c.number() == n)
    }
}

/// An error as sent in a response.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{}: {message}", code.name())]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Wire {
    code: i64,
    message: String,
}

impl Serialize for RpcError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            code: self.code.number(),
            message: self.message.clone(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for RpcError {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = Wire::deserialize(d)?;
        let code = ErrorCode::from_number(w.code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown error code {}", w.code)))?;
        Ok(RpcError {
            code,
            message: w.message,
        })
    }
}
