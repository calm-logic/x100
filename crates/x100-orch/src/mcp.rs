//! The daemon's tools served to the real harnesses over MCP: streamable HTTP on a loopback port
//! the system chooses, one address per driver, `/drivers/<driver id>/mcp`, with the driver's
//! token as a bearer token.
//!
//! The protocol is the official SDK's (`rmcp`), served by hyper with no web framework. Requests are
//! stateless and answered as plain JSON: every request carries who is calling, so there is no
//! session to keep, and a harness's client needs nothing more. What a caller may do is decided by
//! the [`Desk`], the same way for every transport; this module only carries calls to it.

use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::Service as _;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use x100_core::id::DriverId;

use crate::Desk;
use crate::tools::{Tool, schema};

const INSTRUCTIONS: &str = "The x100 daemon's tools. You are one agent of a team: your \
     instructions and each turn's input name your task, your id and what reached your inbox. Act \
     on the team and the project through these tools. After `report` or `wait`, end your turn.";

/// The tools server, listening until it is dropped.
pub struct ToolServer {
    url: String,
    task: JoinHandle<()>,
}

impl Drop for ToolServer {
    fn drop(&mut self) {
        // Aborting the accept loop drops its connections with it.
        self.task.abort();
    }
}

impl ToolServer {
    /// Listens on a loopback port the system chooses and serves `desk`'s tools there. Drivers
    /// started from now on are given an address on it.
    pub async fn start(desk: Arc<Desk>) -> std::io::Result<ToolServer> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let url = format!("http://{}", listener.local_addr()?);
        desk.listening_at(url.clone());
        let config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true);
        let mcp = StreamableHttpService::new(
            move || {
                Ok(Session {
                    desk: Arc::clone(&desk),
                })
            },
            Arc::new(NeverSessionManager::default()),
            config,
        );
        let task = tokio::spawn(accept(listener, TowerToHyperService::new(mcp)));
        Ok(ToolServer { url, task })
    }

    /// `http://127.0.0.1:<port>`, under which each driver has its own address.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Stops listening, and returns once the port is closed.
    pub async fn stop(mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

type Mcp = TowerToHyperService<StreamableHttpService<Session, NeverSessionManager>>;
type Response = http::Response<BoxBody<Bytes, Infallible>>;

async fn accept(listener: TcpListener, mcp: Mcp) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let mcp = mcp.clone();
                    let service = hyper::service::service_fn(move |req| route(mcp.clone(), req));
                    connections.spawn(async move {
                        let _ = http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
                Err(e) => eprintln!("x100: the tools server could not accept a connection: {e}"),
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

/// Who the request says is calling. The desk decides whether it may.
#[derive(Debug, Clone)]
struct Caller {
    driver: DriverId,
    token: String,
}

/// Finds the driver from the path and its token from the header, and hands the request on.
async fn route(mcp: Mcp, mut req: http::Request<Incoming>) -> Result<Response, Infallible> {
    let driver = req
        .uri()
        .path()
        .strip_prefix("/drivers/")
        .and_then(|rest| rest.strip_suffix("/mcp"))
        .and_then(|id| DriverId::from_str(id).ok());
    let Some(driver) = driver else {
        return Ok(plain(
            http::StatusCode::NOT_FOUND,
            "each driver's tools are at /drivers/<driver id>/mcp",
        ));
    };
    let token = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    req.extensions_mut().insert(Caller { driver, token });
    mcp.call(req).await
}

fn plain(status: http::StatusCode, body: &'static str) -> Response {
    let mut response = http::Response::new(Full::new(Bytes::from_static(body.as_bytes())).boxed());
    *response.status_mut() = status;
    response
}

/// One request's handler. Requests are stateless, so it lives as long as the request.
struct Session {
    desk: Arc<Desk>,
}

fn caller(cx: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
    cx.extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Caller>())
        .cloned()
        .ok_or_else(|| ErrorData::internal_error("the request did not say who is calling", None))
}

fn listed(tool: Tool) -> rmcp::model::Tool {
    let Value::Object(schema) = schema(tool) else {
        unreachable!("every tool's schema is an object")
    };
    rmcp::model::Tool::new(tool.name(), tool.description(), schema)
}

fn result(text: String, is_error: bool) -> CallToolResponse {
    let content = vec![ContentBlock::text(text)];
    if is_error {
        CallToolResult::error(content).into()
    } else {
        CallToolResult::success(content).into()
    }
}

impl ServerHandler for Session {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("x100", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        cx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let caller = caller(&cx)?;
        match self.desk.listing(caller.driver, &caller.token).await {
            Ok(tools) => Ok(ListToolsResult::with_all_items(
                tools.into_iter().map(listed).collect(),
            )),
            Err(why) => Err(ErrorData::invalid_request(why, None)),
        }
    }

    /// Every answer, a refusal included, is a tool result the model reads: a protocol error
    /// would reach it as a bare failure, without the reason.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        cx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let caller = caller(&cx)?;
        let Some(tool) = Tool::named(&request.name) else {
            return Ok(result(format!("there is no tool `{}`", request.name), true));
        };
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let call = match tool.parse(arguments) {
            Ok(call) => call,
            Err(why) => return Ok(result(why, true)),
        };
        Ok(
            match self.desk.call(caller.driver, &caller.token, call).await {
                Ok(reply) => result(reply, false),
                Err(why) => result(why, true),
            },
        )
    }
}
