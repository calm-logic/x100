//! JSON-RPC 2.0, one JSON object per line.
//!
//! A line is a request (`id` and `method`), a response (`id` and `result` or `error`) or a
//! notification (`method`, no `id`). JSON-RPC batches are not used. A result is never `null`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ErrorCode, RpcError};
use crate::methods::{Call, Method, Notice};

/// A request's id, which its response repeats.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(u64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: Id,
    pub call: Call,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// `None` only for an error to a line whose id could not be read.
    pub id: Option<Id>,
    pub outcome: Result<Value, RpcError>,
}

impl Response {
    pub fn ok<M: Method>(id: Id, result: &M::Result) -> Response {
        Response {
            id: Some(id),
            outcome: Ok(serde_json::to_value(result).expect("results serialise")),
        }
    }

    pub fn err(id: Option<Id>, error: RpcError) -> Response {
        Response {
            id,
            outcome: Err(error),
        }
    }

    /// The typed result of method `M`, or the error the daemon sent.
    pub fn into_result<M: Method>(self) -> Result<M::Result, RpcError> {
        serde_json::from_value(self.outcome?).map_err(|e| {
            RpcError::new(
                ErrorCode::Internal,
                format!("a malformed result of {}: {e}", M::NAME),
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notice(Notice),
}

/// A line that could not be read, with the request id if that much was readable, so the daemon can
/// answer it.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{error}")]
pub struct BadLine {
    pub id: Option<Id>,
    pub error: RpcError,
}

#[derive(Serialize, Deserialize)]
struct Raw {
    jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

const VERSION: &str = "2.0";

impl Message {
    /// One line, without the newline that ends it.
    pub fn to_line(&self) -> String {
        let blank = Raw {
            jsonrpc: VERSION.into(),
            id: None,
            method: None,
            params: None,
            result: None,
            error: None,
        };
        let raw = match self {
            Message::Request(r) => Raw {
                id: Some(r.id.clone()),
                method: Some(r.call.method().into()),
                params: Some(r.call.params()),
                ..blank
            },
            Message::Notice(n) => Raw {
                method: Some(n.method().into()),
                params: Some(n.params()),
                ..blank
            },
            Message::Response(r) => match &r.outcome {
                Ok(v) => Raw {
                    id: r.id.clone(),
                    result: Some(v.clone()),
                    ..blank
                },
                Err(e) => Raw {
                    id: r.id.clone(),
                    error: Some(e.clone()),
                    ..blank
                },
            },
        };
        // serde_json writes no raw newline: line breaks inside strings are escaped.
        serde_json::to_string(&raw).expect("a message serialises")
    }

    pub fn parse(line: &str) -> Result<Message, BadLine> {
        let bad = |id, code, msg: String| BadLine {
            id,
            error: RpcError::new(code, msg),
        };
        let value: Value = serde_json::from_str(line)
            .map_err(|e| bad(None, ErrorCode::ParseError, e.to_string()))?;
        let raw: Raw = serde_json::from_value(value)
            .map_err(|e| bad(None, ErrorCode::InvalidRequest, e.to_string()))?;
        if raw.jsonrpc != VERSION {
            return Err(bad(
                raw.id,
                ErrorCode::InvalidRequest,
                format!("jsonrpc must be \"{VERSION}\""),
            ));
        }
        match (raw.method, raw.id, raw.result, raw.error) {
            (Some(method), Some(id), None, None) => {
                let params = raw
                    .params
                    .unwrap_or_else(|| Value::Object(Default::default()));
                match Call::parse(&method, params) {
                    Ok(call) => Ok(Message::Request(Request { id, call })),
                    Err(error) => Err(BadLine {
                        id: Some(id),
                        error,
                    }),
                }
            }
            (Some(method), None, None, None) => {
                let params = raw
                    .params
                    .unwrap_or_else(|| Value::Object(Default::default()));
                Notice::parse(&method, params)
                    .map(Message::Notice)
                    .map_err(|error| BadLine { id: None, error })
            }
            (None, id, Some(result), None) => Ok(Message::Response(Response {
                id,
                outcome: Ok(result),
            })),
            (None, id, None, Some(error)) => Ok(Message::Response(Response {
                id,
                outcome: Err(error),
            })),
            (_, id, _, _) => Err(bad(
                id,
                ErrorCode::InvalidRequest,
                "not a request, a response or a notification".into(),
            )),
        }
    }
}
