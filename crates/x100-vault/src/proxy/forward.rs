//! Serving one route: authorising, tunnelling, swapping references, forwarding, redacting.

use std::convert::Infallible;
use std::future::Future;
use std::sync::{Arc, Mutex};

use base64::Engine;
use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full, Limited};
use hyper::body::Incoming;
use hyper::client::conn::http1::SendRequest;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use x100_core::credential::{UseOutcome, Via};
use x100_core::id::CredentialRef;

use super::body::{BoxError, Transform, Transformed};
use super::{EgressOutcome, EgressReport, Shared};
use crate::refs::{self, Decide, Swapper};
use crate::scope::Requester;
use crate::store::Denied;

type Body = BoxBody<Bytes, BoxError>;
type Upstream = tokio::sync::Mutex<Option<SendRequest<Body>>>;

/// A request body up to this size is read whole, so the length it goes out with stays exact
/// after references are swapped. A larger one with a declared length passes through unchanged,
/// so a large upload keeps its length; a chunked one is swapped as it streams.
const BUFFER_LIMIT: usize = 4 << 20;

pub(crate) struct RouteCtx {
    pub(crate) shared: Arc<Shared>,
    pub(crate) requester: Requester,
    pub(crate) token: String,
    pub(crate) pass_through: std::collections::BTreeSet<String>,
    pub(crate) closed: watch::Receiver<()>,
}

impl RouteCtx {
    /// Runs `work` until it ends or the route is dropped.
    fn spawn(&self, work: impl Future<Output = ()> + Send + 'static) {
        let mut closed = self.closed.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = closed.changed() => {}
                () = work => {}
            }
        });
    }
}

struct Target {
    tls: bool,
    host: String,
    port: u16,
}

pub(crate) async fn accept(listener: TcpListener, ctx: Arc<RouteCtx>) {
    while let Ok((stream, _)) = listener.accept().await {
        let conn = Arc::clone(&ctx);
        ctx.spawn(async move {
            let service = service_fn(move |req| {
                let ctx = Arc::clone(&conn);
                async move { Ok::<_, Infallible>(outer(req, ctx).await) }
            });
            let _ = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades()
                .await;
        });
    }
}

async fn outer(req: Request<Incoming>, ctx: Arc<RouteCtx>) -> Response<Body> {
    if !authorised(req.headers(), &ctx.token) {
        let mut response = plain(
            StatusCode::PROXY_AUTHENTICATION_REQUIRED,
            "proxy token required",
        );
        response.headers_mut().insert(
            header::PROXY_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"x100\""),
        );
        return response;
    }
    if req.method() == Method::CONNECT {
        let Some((host, port)) = req
            .uri()
            .authority()
            .map(|a| (a.host().to_owned(), a.port_u16().unwrap_or(443)))
        else {
            return plain(StatusCode::BAD_REQUEST, "CONNECT needs host:port");
        };
        if ctx.shared.is_refused(&host) {
            let report = Report::new(&ctx, "CONNECT", &host, port);
            report.finish(EgressOutcome::HostRefused);
            return plain(StatusCode::FORBIDDEN, "host refused");
        }
        if ctx.pass_through.contains(&host.to_ascii_lowercase()) {
            return relay(
                req,
                Target {
                    tls: true,
                    host,
                    port,
                },
                &ctx,
            )
            .await;
        }
        let tunnel_ctx = Arc::clone(&ctx);
        ctx.spawn(async move {
            if let Ok(upgraded) = hyper::upgrade::on(req).await {
                tunnel(
                    upgraded,
                    Target {
                        tls: true,
                        host,
                        port,
                    },
                    tunnel_ctx,
                )
                .await;
            }
        });
        return Response::new(empty());
    }
    let uri = req.uri();
    match (uri.scheme_str(), uri.host()) {
        (Some("http"), Some(host)) => {
            let target = Target {
                tls: false,
                host: host.to_owned(),
                port: uri.port_u16().unwrap_or(80),
            };
            forward(req, &target, &ctx, &Upstream::default()).await
        }
        _ => plain(
            StatusCode::BAD_REQUEST,
            "send absolute-form http:// requests or CONNECT",
        ),
    }
}

/// `Proxy-Authorization: Basic` with any user name and the route's token as the password.
fn authorised(headers: &HeaderMap, token: &str) -> bool {
    let Some(given) = headers
        .get(header::PROXY_AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| {
            base64::engine::general_purpose::STANDARD
                .decode(v.trim())
                .ok()
        })
    else {
        return false;
    };
    let password = given
        .iter()
        .position(|&b| b == b':')
        .map_or(&given[..0], |i| &given[i + 1..]);
    // Compared in constant time, since the sandbox could otherwise time its guesses.
    password.len() == token.len()
        && password
            .iter()
            .zip(token.as_bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Connects first, so a host that cannot be reached is a 502 rather than a tunnel that closes,
/// then splices the client's bytes to it.
async fn relay(req: Request<Incoming>, target: Target, ctx: &Arc<RouteCtx>) -> Response<Body> {
    let report = Report::new(ctx, "CONNECT", &target.host, target.port);
    let mut upstream = match tokio::time::timeout(CONNECT_TIMEOUT, tcp(&target, ctx)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(e)) => {
            report.finish(EgressOutcome::Failed(e));
            return plain(
                StatusCode::BAD_GATEWAY,
                "the proxy could not reach the host",
            );
        }
        Err(_) => {
            report.finish(EgressOutcome::Failed("connecting timed out".into()));
            return plain(
                StatusCode::GATEWAY_TIMEOUT,
                "the proxy could not reach the host",
            );
        }
    };
    report.finish(EgressOutcome::Tunnelled);
    ctx.spawn(async move {
        if let Ok(upgraded) = hyper::upgrade::on(req).await {
            let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(upgraded), &mut upstream).await;
        }
    });
    Response::new(empty())
}

async fn tcp(target: &Target, ctx: &RouteCtx) -> Result<TcpStream, String> {
    match ctx.shared.resolve.get(&target.host.to_ascii_lowercase()) {
        Some(ip) => TcpStream::connect((*ip, target.port)).await,
        None => TcpStream::connect((target.host.as_str(), target.port)).await,
    }
    .map_err(|e| format!("connecting to {}:{}: {e}", target.host, target.port))
}

async fn tunnel(upgraded: hyper::upgrade::Upgraded, target: Target, ctx: Arc<RouteCtx>) {
    let Ok(config) = ctx.shared.authority.server_config(&target.host) else {
        return;
    };
    let Ok(tls) = TlsAcceptor::from(config)
        .accept(TokioIo::new(upgraded))
        .await
    else {
        return;
    };
    let target = Arc::new(target);
    let upstream = Arc::new(Upstream::default());
    let service = service_fn(move |req| {
        let (target, ctx, upstream) =
            (Arc::clone(&target), Arc::clone(&ctx), Arc::clone(&upstream));
        async move { Ok::<_, Infallible>(forward(req, &target, &ctx, &upstream).await) }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(tls), service)
        .await;
}

async fn forward(
    req: Request<Incoming>,
    target: &Target,
    ctx: &RouteCtx,
    upstream: &Upstream,
) -> Response<Body> {
    let report = Report::new(ctx, req.method().as_str(), &target.host, target.port);
    if ctx.shared.is_refused(&target.host) {
        report.finish(EgressOutcome::HostRefused);
        return plain(StatusCode::FORBIDDEN, "host refused");
    }
    let head_request = req.method() == Method::HEAD;
    let (mut parts, body) = req.into_parts();
    let mut decide = Swap {
        shared: Arc::clone(&ctx.shared),
        requester: ctx.requester,
        via: Via::Proxy {
            host: target.host.clone(),
        },
        report: Arc::clone(&report.0),
    };
    strip_hop_by_hop(&mut parts.headers);
    swap_headers(&mut parts.headers, &mut decide);
    // The response is redacted on its way back, which a compressed body would hide from it.
    parts.headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    parts.uri = match origin_form(&parts.uri, &mut decide) {
        Some(uri) => uri,
        None => return plain(StatusCode::BAD_REQUEST, "bad request target"),
    };
    let body = match request_body(&mut parts.headers, body, &mut decide).await {
        Ok(body) => body,
        Err(e) => {
            report.finish(EgressOutcome::Failed(e.clone()));
            return plain(StatusCode::BAD_REQUEST, &e);
        }
    };
    drop(decide);
    let response = send(upstream, target, ctx, Request::from_parts(parts, body)).await;
    match response {
        Ok(response) => {
            report.finish(EgressOutcome::Answered(response.status().as_u16()));
            redact_response(response, head_request, ctx)
        }
        Err(e) => {
            report.finish(EgressOutcome::Failed(e));
            plain(
                StatusCode::BAD_GATEWAY,
                "the proxy could not reach the host",
            )
        }
    }
}

async fn request_body(
    headers: &mut HeaderMap,
    body: Incoming,
    decide: &mut Swap,
) -> Result<Body, String> {
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    match length {
        Some(n) if n <= BUFFER_LIMIT => {
            let bytes = Limited::new(body, BUFFER_LIMIT)
                .collect()
                .await
                .map_err(|e| format!("reading the request body: {e}"))?
                .to_bytes();
            let swapped = refs::swap_all(&bytes, decide);
            headers.insert(header::CONTENT_LENGTH, HeaderValue::from(swapped.len()));
            Ok(Full::new(Bytes::from(swapped))
                .map_err(|e| match e {})
                .boxed())
        }
        Some(_) => Ok(body.map_err(BoxError::from).boxed()),
        None if hyper::body::Body::is_end_stream(&body) => Ok(empty()),
        None => Ok(Transformed::new(
            body,
            BodySwap {
                swapper: Swapper::default(),
                decide: decide.clone(),
            },
        )
        .boxed()),
    }
}

fn redact_response(
    response: Response<Incoming>,
    head_request: bool,
    ctx: &RouteCtx,
) -> Response<Body> {
    let (mut parts, body) = response.into_parts();
    strip_hop_by_hop(&mut parts.headers);
    let redactor = ctx.shared.vault.redactor();
    for value in parts.headers.values_mut() {
        let redacted = redactor.redact(value.as_bytes());
        if redacted != value.as_bytes()
            && let Ok(v) = HeaderValue::from_bytes(&redacted)
        {
            *value = v;
        }
    }
    let bodiless = head_request
        || parts.status == StatusCode::NO_CONTENT
        || parts.status == StatusCode::NOT_MODIFIED;
    if bodiless {
        return Response::from_parts(parts, body.map_err(BoxError::from).boxed());
    }
    // Redaction can change the length, so the body goes back chunked.
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Transformed::new(body, redactor.stream()).boxed())
}

async fn send(
    upstream: &Upstream,
    target: &Target,
    ctx: &RouteCtx,
    req: Request<Body>,
) -> Result<Response<Incoming>, String> {
    let mut slot = upstream.lock().await;
    if let Some(sender) = slot.as_mut()
        && sender.ready().await.is_err()
    {
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(connect(target, ctx).await?);
    }
    let sender = slot.as_mut().expect("connected above");
    sender
        .send_request(req)
        .await
        .map_err(|e| format!("upstream: {e}"))
}

/// A host that never answers fails the request after this, rather than holding the driver.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn connect(target: &Target, ctx: &RouteCtx) -> Result<SendRequest<Body>, String> {
    tokio::time::timeout(CONNECT_TIMEOUT, connect_now(target, ctx))
        .await
        .map_err(|_| format!("connecting to {}:{} timed out", target.host, target.port))?
}

async fn connect_now(target: &Target, ctx: &RouteCtx) -> Result<SendRequest<Body>, String> {
    let tcp = tcp(target, ctx).await?;
    if !target.tls {
        return handshake(TokioIo::new(tcp), ctx).await;
    }
    let name = rustls::pki_types::ServerName::try_from(target.host.clone())
        .map_err(|e| format!("{}: {e}", target.host))?;
    let tls = TlsConnector::from(Arc::clone(&ctx.shared.client))
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS to {}: {e}", target.host))?;
    handshake(TokioIo::new(tls), ctx).await
}

async fn handshake<IO>(io: IO, ctx: &RouteCtx) -> Result<SendRequest<Body>, String>
where
    IO: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| format!("upstream handshake: {e}"))?;
    ctx.spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

/// Headers that belong to one connection and are not forwarded, by RFC 9110.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        header::CONNECTION,
        header::PROXY_AUTHORIZATION,
        header::PROXY_AUTHENTICATE,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
    ] {
        headers.remove(name);
    }
    headers.remove("proxy-connection");
    headers.remove("keep-alive");
}

fn swap_headers(headers: &mut HeaderMap, decide: &mut Swap) {
    for value in headers.values_mut() {
        if memchr::memmem::find(value.as_bytes(), b"ref_").is_none() {
            continue;
        }
        let swapped = refs::swap_all(value.as_bytes(), decide);
        // A value that cannot be a header (it holds a line break) leaves the reference in place.
        if let Ok(v) = HeaderValue::from_bytes(&swapped) {
            *value = v;
        }
    }
}

/// The path and query to send upstream, with references in the query swapped.
fn origin_form(uri: &Uri, decide: &mut Swap) -> Option<Uri> {
    let pq = uri.path_and_query().map_or("/", |pq| pq.as_str());
    let (path, query) = match pq.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (pq, None),
    };
    let mut out = path.as_bytes().to_vec();
    if let Some(query) = query {
        out.push(b'?');
        out.extend(refs::swap_all(query.as_bytes(), decide));
    }
    Uri::try_from(out).ok()
}

/// Swaps references for values the credential allows, and records each outcome.
#[derive(Clone)]
struct Swap {
    shared: Arc<Shared>,
    requester: Requester,
    via: Via,
    report: Arc<Pending>,
}

impl Decide for Swap {
    fn decide(&mut self, reference: CredentialRef, out: &mut Vec<u8>) -> bool {
        match self
            .shared
            .vault
            .take(reference, &self.requester, &self.via)
        {
            Ok(value) => {
                out.extend_from_slice(value.expose());
                self.report.record(reference, UseOutcome::Injected);
                true
            }
            Err(Denied::Refused(refusal)) => {
                self.report.record(reference, UseOutcome::Refused(refusal));
                false
            }
            Err(Denied::Unknown) => false,
        }
    }
}

struct BodySwap {
    swapper: Swapper,
    decide: Swap,
}

impl Transform for BodySwap {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.swapper.push(chunk, &mut self.decide)
    }

    fn finish(&mut self) -> Vec<u8> {
        self.swapper.finish(&mut self.decide)
    }
}

/// A report under construction. It is sent when the last holder lets go: the handler once the
/// response head is in, and a streaming request body once it has gone out.
struct Pending {
    shared: Arc<Shared>,
    report: Mutex<EgressReport>,
}

impl Pending {
    fn record(&self, reference: CredentialRef, outcome: UseOutcome) {
        let mut report = self.report.lock().expect("report");
        if !report
            .uses
            .iter()
            .any(|(r, o)| *r == reference && *o == outcome)
        {
            report.uses.push((reference, outcome));
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Ok(report) = self.report.get_mut() {
            let _ = self.shared.reports.send(report.clone());
        }
    }
}

struct Report(Arc<Pending>);

impl Report {
    fn new(ctx: &RouteCtx, method: &str, host: &str, port: u16) -> Self {
        Self(Arc::new(Pending {
            shared: Arc::clone(&ctx.shared),
            report: Mutex::new(EgressReport {
                requester: ctx.requester,
                method: method.to_owned(),
                host: host.to_owned(),
                port,
                uses: Vec::new(),
                outcome: EgressOutcome::Failed("the request was abandoned".into()),
            }),
        }))
    }

    fn finish(self, outcome: EgressOutcome) {
        self.0.report.lock().expect("report").outcome = outcome;
    }
}

fn empty() -> Body {
    Empty::new().map_err(|e| match e {}).boxed()
}

fn plain(status: StatusCode, text: &str) -> Response<Body> {
    let mut response = Response::new(
        Full::new(Bytes::from(text.to_owned()))
            .map_err(|e| match e {})
            .boxed(),
    );
    *response.status_mut() = status;
    response
}
