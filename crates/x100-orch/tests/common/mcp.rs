//! A small MCP client over real HTTP, speaking as a harness's client does: JSON-RPC posted to a
//! driver's own address with its bearer token, accepting JSON or an event stream.

use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use x100_drivers::ToolEndpoint;

pub const PROTOCOL: &str = "2025-06-18";

pub struct Client {
    url: http::Uri,
    token: String,
    next: AtomicU64,
}

/// What a `tools/call` answered: the text the model would read, and whether it is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub text: String,
    pub is_error: bool,
}

impl Client {
    pub fn new(endpoint: &ToolEndpoint) -> Client {
        Client::at(&endpoint.url, &endpoint.token)
    }

    pub fn at(url: &str, token: &str) -> Client {
        Client {
            url: url.parse().expect("an address"),
            token: token.to_owned(),
            next: AtomicU64::new(1),
        }
    }

    /// The JSON-RPC response to a request, with its HTTP status.
    pub async fn request(&self, method: &str, params: Value) -> (http::StatusCode, Value) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let (status, bytes) = self.post(body).await;
        (status, message(&bytes))
    }

    pub async fn notify(&self, method: &str) -> http::StatusCode {
        self.post(json!({ "jsonrpc": "2.0", "method": method }))
            .await
            .0
    }

    /// The handshake a harness makes first. Returns the server's answer.
    pub async fn initialize(&self) -> Value {
        let (status, answer) = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL,
                    "capabilities": {},
                    "clientInfo": { "name": "x100-test", "version": "0" },
                }),
            )
            .await;
        assert_eq!(status, http::StatusCode::OK, "{answer}");
        assert!(
            self.notify("notifications/initialized").await.is_success(),
            "the server took the initialized notice"
        );
        answer["result"].clone()
    }

    /// The tools listed, each as the server described it, or the error it answered with.
    pub async fn tools(&self) -> Result<Vec<Value>, Value> {
        let (_, answer) = self.request("tools/list", json!({})).await;
        match answer["result"]["tools"].as_array() {
            Some(tools) => Ok(tools.clone()),
            None => Err(answer["error"].clone()),
        }
    }

    pub async fn names(&self) -> Vec<String> {
        self.tools()
            .await
            .expect("the tools are listed")
            .iter()
            .map(|t| t["name"].as_str().expect("a name").to_owned())
            .collect()
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Answer {
        let (status, answer) = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await;
        assert_eq!(status, http::StatusCode::OK, "{answer}");
        let result = &answer["result"];
        let text = result["content"]
            .as_array()
            .unwrap_or_else(|| panic!("a tool result: {answer}"))
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Answer {
            text,
            is_error: result["isError"].as_bool().unwrap_or(false),
        }
    }

    async fn post(&self, body: Value) -> (http::StatusCode, Bytes) {
        let authority = self.url.authority().expect("a host and port").clone();
        let stream = TcpStream::connect(authority.as_str())
            .await
            .expect("the tools server is listening");
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .expect("an HTTP connection");
        tokio::spawn(connection);
        let request = http::Request::post(self.url.path())
            .header(http::header::HOST, authority.as_str())
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(http::header::ACCEPT, "application/json, text/event-stream")
            .header(
                http::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .header("mcp-protocol-version", PROTOCOL)
            .body(Full::new(Bytes::from(body.to_string())))
            .expect("a request");
        let response = sender.send_request(request).await.expect("a response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("the body")
            .to_bytes();
        (status, bytes)
    }
}

/// The one JSON-RPC message in a body that is JSON, or the last in an event stream.
fn message(bytes: &[u8]) -> Value {
    let text = String::from_utf8_lossy(bytes);
    if let Ok(v) = serde_json::from_str(&text) {
        return v;
    }
    text.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str(d.trim()).ok())
        .next_back()
        .unwrap_or_else(|| json!({ "unparsed": text }))
}
