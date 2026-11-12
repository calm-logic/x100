//! A fake model provider on localhost, so the real harnesses can be driven end to end at no cost.
//!
//! It speaks enough of the Anthropic Messages API (`POST /v1/messages`, streaming) and the OpenAI
//! Responses API (`POST /v1/responses`, streaming) to play a queue of [`Reply`] values, one per
//! model request, and records every request. It also serves a minimal MCP endpoint at `/mcp` with
//! one tool, `ping`, standing in for the daemon's tools, and refuses and records any `CONNECT`, so
//! a harness pointed at it as its proxy cannot reach anything else.

mod http;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

/// Tokens every reply reports: fresh input, cache reads and writes, and output.
pub const REPLY_INPUT: u64 = 100;
pub const REPLY_CACHE_READ: u64 = 30;
/// Anthropic replies only: OpenAI's usage has no cache writes.
pub const REPLY_CACHE_WRITE: u64 = 20;
pub const REPLY_OUTPUT: u64 = 7;

/// What the provider answers to the next model request.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Text(String),
    /// An Anthropic `tool_use`, or an OpenAI `function_call` whose arguments are `input`.
    Tool {
        name: String,
        input: Value,
    },
    /// An OpenAI custom tool call with free-form input, such as Codex's `apply_patch`.
    Freeform {
        name: String,
        input: String,
    },
    /// An HTTP error with the provider's error body.
    Status(u16),
}

/// A model request the provider received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A JSON-RPC message the MCP endpoint received.
#[derive(Debug, Clone)]
pub struct McpCall {
    pub method: String,
    pub authorization: Option<String>,
    pub params: Value,
}

#[derive(Default)]
struct Shared {
    replies: VecDeque<Reply>,
    requests: Vec<Recorded>,
    mcp: Vec<McpCall>,
    connects: Vec<String>,
    next_id: u64,
}

pub struct FakeProvider {
    addr: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    server: JoinHandle<()>,
}

impl Drop for FakeProvider {
    fn drop(&mut self) {
        // Aborting the server drops its connections, including any held open.
        self.server.abort();
    }
}

impl FakeProvider {
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let state = Arc::clone(&shared);
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((stream, _)) = listener.accept().await {
                connections.spawn(serve(stream, Arc::clone(&state)));
                while connections.try_join_next().is_some() {}
            }
        });
        Ok(Self {
            addr,
            shared,
            server,
        })
    }

    /// The base URL, `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.url())
    }

    /// Queues the answer to a later model request, in order.
    pub fn reply(&self, reply: Reply) {
        self.shared.lock().expect("lock").replies.push_back(reply);
    }

    /// Replies queued and not yet used.
    pub fn unused(&self) -> usize {
        self.shared.lock().expect("lock").replies.len()
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.shared.lock().expect("lock").requests.clone()
    }

    pub fn mcp_calls(&self) -> Vec<McpCall> {
        self.shared.lock().expect("lock").mcp.clone()
    }

    /// Every host a client tried to reach through this server as its proxy.
    pub fn proxy_attempts(&self) -> Vec<String> {
        self.shared.lock().expect("lock").connects.clone()
    }
}

async fn serve(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) {
    let Ok(request) = http::read_request(&mut stream).await else {
        return;
    };
    let (status, content_type, body) = answer(&request, &shared);
    let _ = http::respond(&mut stream, status, content_type, &body).await;
}

fn answer(request: &http::Request, shared: &Mutex<Shared>) -> (u16, &'static str, Vec<u8>) {
    const JSON: &str = "application/json";
    const SSE: &str = "text/event-stream";
    let mut shared = shared.lock().expect("lock");
    if request.method == "CONNECT" {
        shared.connects.push(request.target.clone());
        return (403, JSON, Vec::new());
    }
    let path = request
        .target
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    if path.starts_with("/mcp") {
        return match request.method.as_str() {
            "POST" => mcp(request, body, &mut shared),
            _ => (405, JSON, Vec::new()),
        };
    }
    if request.method != "POST" {
        return (404, JSON, Vec::new());
    }
    if path.ends_with("/count_tokens") {
        return (
            200,
            JSON,
            json!({ "input_tokens": 1000 }).to_string().into_bytes(),
        );
    }
    let anthropic = path.ends_with("/messages");
    let openai = path.ends_with("/responses");
    if !anthropic && !openai {
        return (404, JSON, Vec::new());
    }
    if anthropic && body["stream"] != true {
        // Side requests, such as a title, are answered without using the queue.
        let message = json!({
            "id": "msg_side", "type": "message", "role": "assistant", "model": body["model"],
            "content": [{ "type": "text", "text": "ok" }], "stop_reason": "end_turn",
            "stop_sequence": null, "usage": { "input_tokens": 1, "output_tokens": 1 },
        });
        return (200, JSON, message.to_string().into_bytes());
    }
    shared.requests.push(Recorded {
        path,
        headers: request.headers.clone(),
        body: body.clone(),
    });
    shared.next_id += 1;
    let n = shared.next_id;
    let Some(reply) = shared.replies.pop_front() else {
        let error = json!({ "error": { "type": "x100_fake", "message": "the fake provider has no reply queued" } });
        return (400, JSON, error.to_string().into_bytes());
    };
    let model = body["model"].clone();
    match (reply, anthropic) {
        (Reply::Status(status), true) => (status, JSON, anthropic_error(status)),
        (Reply::Status(status), false) => (status, JSON, openai_error(status)),
        (reply, true) => (200, SSE, anthropic_stream(&reply, &model, n)),
        (reply, false) => (200, SSE, openai_stream(&reply, &model, n)),
    }
}

fn mcp(request: &http::Request, body: Value, shared: &mut Shared) -> (u16, &'static str, Vec<u8>) {
    let method = body["method"].as_str().unwrap_or_default().to_owned();
    shared.mcp.push(McpCall {
        method: method.clone(),
        authorization: request.header("authorization").map(str::to_owned),
        params: body["params"].clone(),
    });
    let Some(id) = body.get("id").cloned() else {
        return (202, "application/json", Vec::new());
    };
    let reply = match method.as_str() {
        "initialize" => json!({ "jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": body["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "x100", "version": "0" },
        }}),
        "tools/list" => json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": [{
            "name": "ping",
            "description": "Answers pong.",
            "inputSchema": { "type": "object", "properties": { "msg": { "type": "string" } } },
        }]}}),
        "tools/call" => json!({ "jsonrpc": "2.0", "id": id, "result": {
            "content": [{ "type": "text", "text": "pong" }],
        }}),
        _ => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "not found" } })
        }
    };
    (200, "application/json", reply.to_string().into_bytes())
}

fn event(name: &str, data: &Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

fn anthropic_stream(reply: &Reply, model: &Value, n: u64) -> Vec<u8> {
    let mut out = event(
        "message_start",
        &json!({ "type": "message_start", "message": {
            "id": format!("msg_{n}"), "type": "message", "role": "assistant", "model": model,
            "content": [], "stop_reason": null, "stop_sequence": null,
            "usage": {
                "input_tokens": REPLY_INPUT, "output_tokens": 1,
                "cache_creation_input_tokens": REPLY_CACHE_WRITE,
                "cache_read_input_tokens": REPLY_CACHE_READ,
            },
        }}),
    );
    let (block, delta, stop) = match reply {
        Reply::Tool { name, input } => (
            json!({ "type": "tool_use", "id": format!("toolu_{n}"), "name": name, "input": {} }),
            json!({ "type": "input_json_delta", "partial_json": input.to_string() }),
            "tool_use",
        ),
        Reply::Text(text) => (
            json!({ "type": "text", "text": "" }),
            json!({ "type": "text_delta", "text": text }),
            "end_turn",
        ),
        Reply::Freeform { name, input } => (
            json!({ "type": "tool_use", "id": format!("toolu_{n}"), "name": name, "input": {} }),
            json!({ "type": "input_json_delta", "partial_json": json!({ "input": input }).to_string() }),
            "tool_use",
        ),
        Reply::Status(_) => unreachable!("errors are not streamed"),
    };
    out += &event(
        "content_block_start",
        &json!({ "type": "content_block_start", "index": 0, "content_block": block }),
    );
    out += &event(
        "content_block_delta",
        &json!({ "type": "content_block_delta", "index": 0, "delta": delta }),
    );
    out += &event(
        "content_block_stop",
        &json!({ "type": "content_block_stop", "index": 0 }),
    );
    out += &event(
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop, "stop_sequence": null },
            "usage": { "output_tokens": REPLY_OUTPUT },
        }),
    );
    out += &event("message_stop", &json!({ "type": "message_stop" }));
    out.into_bytes()
}

fn anthropic_error(status: u16) -> Vec<u8> {
    let kind = match status {
        401 => "authentication_error",
        403 => "permission_error",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        400 => "invalid_request_error",
        _ => "api_error",
    };
    json!({ "type": "error", "error": { "type": kind, "message": format!("fake {kind}") } })
        .to_string()
        .into_bytes()
}

fn openai_stream(reply: &Reply, model: &Value, n: u64) -> Vec<u8> {
    let response = json!({ "id": format!("resp_{n}"), "object": "response", "created_at": 1,
        "status": "in_progress", "model": model, "output": [] });
    let mut out = event(
        "response.created",
        &json!({ "type": "response.created", "response": response }),
    );
    let item = match reply {
        Reply::Text(text) => {
            let id = format!("msg_{n}");
            let started = json!({ "type": "message", "id": id, "status": "in_progress", "role": "assistant", "content": [] });
            out += &event(
                "response.output_item.added",
                &json!({ "type": "response.output_item.added", "output_index": 0, "item": started }),
            );
            out += &event(
                "response.output_text.delta",
                &json!({
                    "type": "response.output_text.delta", "item_id": id, "output_index": 0, "content_index": 0, "delta": text,
                }),
            );
            json!({ "type": "message", "id": id, "status": "completed", "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }] })
        }
        Reply::Tool { name, input } => json!({ "type": "function_call", "id": format!("fc_{n}"),
            "call_id": format!("call_{n}"), "name": name, "arguments": input.to_string(), "status": "completed" }),
        Reply::Freeform { name, input } => {
            json!({ "type": "custom_tool_call", "id": format!("ctc_{n}"),
            "call_id": format!("call_{n}"), "name": name, "input": input, "status": "completed" })
        }
        Reply::Status(_) => unreachable!("errors are not streamed"),
    };
    if !matches!(reply, Reply::Text(_)) {
        out += &event(
            "response.output_item.added",
            &json!({ "type": "response.output_item.added", "output_index": 0, "item": item }),
        );
    }
    out += &event(
        "response.output_item.done",
        &json!({ "type": "response.output_item.done", "output_index": 0, "item": item }),
    );
    let done = json!({ "id": format!("resp_{n}"), "object": "response", "created_at": 1,
        "status": "completed", "model": model, "output": [item],
        "usage": {
            "input_tokens": REPLY_INPUT + REPLY_CACHE_READ,
            "input_tokens_details": { "cached_tokens": REPLY_CACHE_READ },
            "output_tokens": REPLY_OUTPUT,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": REPLY_INPUT + REPLY_CACHE_READ + REPLY_OUTPUT,
        } });
    out += &event(
        "response.completed",
        &json!({ "type": "response.completed", "response": done }),
    );
    out.into_bytes()
}

fn openai_error(status: u16) -> Vec<u8> {
    json!({ "error": { "type": "fake", "code": format!("fake_{status}"), "message": format!("fake error {status}") } })
        .to_string()
        .into_bytes()
}
