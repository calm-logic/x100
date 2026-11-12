//! Helpers shared by the integration tests: a test authority for local servers, HTTPS and HTTP
//! servers on loopback, a client that goes through a proxy route, and a search for a value's
//! forms written independently of the redactor's.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use bytes::Bytes;
use http::{Request, Response};
use http_body::{Body, Frame};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use x100_core::credential::{Credential, CredentialScope, Projects};
use x100_core::driver::Purpose;
use x100_core::id::{ActionId, AgentId, CredentialRef, DriverId, ProjectId};
use x100_vault::{
    Admission, Admit, EgressReport, NoKeychain, Proxy, ProxyConfig, Requester, Route, SecretValue,
    UpstreamTrust, Vault,
};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub type TestBody = BoxBody<Bytes, BoxError>;

/// A dummy value invented for these tests. It opens nothing anywhere.
pub const VALUE: &str = "hb_live_9f8e7d6c5b4a3928+1706/f5e4d3c2b1a0=";
pub const ALLOWED: &str = "allowed.x100.test";
pub const OTHER: &str = "other.x100.test";
/// Where the real harnesses' fake provider is reached, behind TLS from the test authority.
pub const PROVIDER: &str = "provider.x100.test";
pub const PROJECT: ProjectId = ProjectId::from_u128(70);

pub fn requester(purpose: Purpose) -> Requester {
    Requester {
        project: PROJECT,
        agent: AgentId::from_u128(80),
        driver: DriverId::from_u128(90),
        purpose,
    }
}

pub fn full(body: impl Into<Bytes>) -> TestBody {
    Full::new(body.into()).map_err(|e| match e {}).boxed()
}

pub fn empty() -> TestBody {
    Empty::new().map_err(|e| match e {}).boxed()
}

/// A body fed from a channel, so a test controls when each chunk is written.
pub struct ChannelBody(pub mpsc::Receiver<Bytes>);

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        self.0.poll_recv(cx).map(|c| c.map(|b| Ok(Frame::data(b))))
    }
}

pub fn channel_body() -> (mpsc::Sender<Bytes>, TestBody) {
    let (tx, rx) = mpsc::channel(4);
    (tx, ChannelBody(rx).boxed())
}

/// A certificate authority for the local test servers, standing in for the public ones.
pub struct TestCa {
    issuer: Issuer<'static, KeyPair>,
    pub cert: CertificateDer<'static>,
}

impl TestCa {
    pub fn new() -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        params
            .distinguished_name
            .push(DnType::CommonName, "x100 test upstream authority");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let cert = params.self_signed(&key).unwrap().der().clone();
        Self {
            issuer: Issuer::new(params, key),
            cert,
        }
    }

    pub fn server_config(&self, hosts: &[&str]) -> Arc<ServerConfig> {
        let key = KeyPair::generate().unwrap();
        let params =
            CertificateParams::new(hosts.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>())
                .unwrap();
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![cert.der().clone()], key)
                .unwrap(),
        )
    }
}

/// A local server; it stops when dropped.
pub struct TestServer {
    pub port: u16,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    pub async fn https<F, Fut>(tls: Option<Arc<ServerConfig>>, handler: F) -> Self
    where
        F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response<TestBody>> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            while let Ok((tcp, _)) = listener.accept().await {
                let handler = handler.clone();
                let tls = tls.clone();
                conns.spawn(async move {
                    let service = service_fn(move |req| {
                        let handler = handler.clone();
                        async move { Ok::<_, Infallible>(handler(req).await) }
                    });
                    let builder = hyper::server::conn::http1::Builder::new();
                    match tls {
                        Some(config) => {
                            if let Ok(stream) = TlsAcceptor::from(config).accept(tcp).await {
                                let _ = builder
                                    .serve_connection(TokioIo::new(stream), service)
                                    .await;
                            }
                        }
                        None => {
                            let _ = builder.serve_connection(TokioIo::new(tcp), service).await;
                        }
                    }
                });
            }
        });
        Self { port, task }
    }
}

/// What a client saw of one exchange.
#[derive(Debug, Default)]
pub struct Seen {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Everything the client saw, as one haystack for the leak search.
    pub fn all(&self) -> Vec<u8> {
        let mut out = format!("{} {:?}\n", self.status, self.headers).into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

pub fn proxy_auth(route: &Route) -> String {
    let url = route.url();
    let userinfo = url
        .trim_start_matches("http://")
        .split('@')
        .next()
        .unwrap()
        .to_owned();
    format!("Basic {}", STANDARD.encode(userinfo))
}

/// A sandboxed client's view: `CONNECT host:port` through the route with its token, TLS that
/// trusts only the proxy's authority, then one request.
pub async fn https_via(
    route: &Route,
    authority: &CertificateDer<'static>,
    host: &str,
    port: u16,
    req: Request<TestBody>,
) -> Seen {
    let response = https_response(route, authority, host, port, req).await;
    read(response).await
}

pub async fn https_response(
    route: &Route,
    authority: &CertificateDer<'static>,
    host: &str,
    port: u16,
    req: Request<TestBody>,
) -> Response<Incoming> {
    let tcp = TcpStream::connect(route.addr()).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
        .await
        .unwrap();
    tokio::spawn(conn.with_upgrades());
    let connect = Request::connect(format!("{host}:{port}"))
        .header(http::header::PROXY_AUTHORIZATION, proxy_auth(route))
        .body(empty())
        .unwrap();
    let answer = sender.send_request(connect).await.unwrap();
    assert_eq!(answer.status(), 200, "CONNECT refused");
    let upgraded = hyper::upgrade::on(answer).await.unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(authority.clone()).unwrap();
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let tls = TlsConnector::from(Arc::new(config))
        .connect(
            ServerName::try_from(host.to_owned()).unwrap(),
            TokioIo::new(upgraded),
        )
        .await
        .expect("the proxy's leaf certificate checks out against its authority");
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .unwrap();
    tokio::spawn(conn);
    sender.send_request(req).await.unwrap()
}

pub async fn read(response: Response<Incoming>) -> Seen {
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Seen {
        status,
        headers,
        body,
    }
}

/// A vault with one credential, an authority, and a proxy that sends the test hosts to
/// loopback and trusts only the test authority upstream.
pub struct Setup {
    pub dir: tempfile::TempDir,
    pub vault: Vault,
    pub proxy: Proxy,
    pub reports: mpsc::UnboundedReceiver<EgressReport>,
    pub reference: CredentialRef,
    pub ca: TestCa,
    pub authority: CertificateDer<'static>,
    /// The test double standing in for the gate.
    pub admitted: Arc<AdmitAll>,
}

/// A test double for the gate: it admits every request, or refuses those to the hosts it is
/// told to, and counts what it was asked.
#[derive(Default)]
pub struct AdmitAll {
    pub asked: Mutex<Vec<(String, String)>>,
    pub refuse: Mutex<Vec<String>>,
}

pub const REFUSED_WHY: &str = "held for the builder";

#[async_trait::async_trait]
impl Admission for AdmitAll {
    async fn admit(&self, _: Requester, method: &str, host: &str) -> Admit {
        let mut asked = self.asked.lock().unwrap();
        asked.push((method.to_owned(), host.to_owned()));
        let action = ActionId::from_u128(asked.len() as u128);
        if self.refuse.lock().unwrap().iter().any(|h| h == host) {
            Admit::Refuse {
                action: Some(action),
                why: REFUSED_WHY.into(),
            }
        } else {
            Admit::Go { action }
        }
    }
}

pub fn scope(hosts: &[&str], projects: Projects) -> CredentialScope {
    CredentialScope {
        hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
        commands: BTreeSet::new(),
        projects,
    }
}

pub fn setup_with(hosts: &[&str], projects: Projects, upstream: Option<UpstreamTrust>) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("vault"), &NoKeychain).unwrap();
    let reference = CredentialRef::from_u128(0x0001_7f3a);
    vault
        .add(
            Credential {
                reference,
                name: "homebox".into(),
                scope: scope(hosts, projects),
            },
            SecretValue::from(VALUE),
        )
        .unwrap();
    let ca = TestCa::new();
    let authority = vault.authority().unwrap();
    let authority_cert = authority.certificate().clone();
    let local = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let config = ProxyConfig {
        upstream: upstream.unwrap_or_else(|| UpstreamTrust::Only(vec![ca.cert.clone()])),
        resolve: [ALLOWED, OTHER, PROVIDER]
            .into_iter()
            .map(|h| (h.to_owned(), local.into()))
            .collect(),
        closed: false,
    };
    let (proxy, reports) = Proxy::new(vault.clone(), authority, config).unwrap();
    let admitted = Arc::new(AdmitAll::default());
    proxy.admit_with(admitted.clone());
    Setup {
        admitted,
        dir,
        vault,
        proxy,
        reports,
        reference,
        ca,
        authority: authority_cert,
    }
}

pub fn setup() -> Setup {
    setup_with(&[ALLOWED], Projects::All, None)
}

impl Setup {
    /// The next report, waiting a short while for it.
    pub async fn report(&mut self) -> EgressReport {
        tokio::time::timeout(Duration::from_secs(10), self.reports.recv())
            .await
            .expect("a report in time")
            .expect("the proxy is alive")
    }
}

/// Every form of `value` a careless or hostile program might print: as it is, percent-encoded
/// by the common rules in either case, hex in either case, and base64 in both alphabets, padded
/// or not, whole or at any alignment inside other data. Written separately from the
/// redactor's own list, so the two check each other.
pub fn forms(value: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![("raw".to_owned(), value.to_vec())];
    let pct = |keep: &[u8], upper: bool| -> Vec<u8> {
        let mut s = String::new();
        for &b in value {
            if b.is_ascii_alphanumeric() || keep.contains(&b) {
                s.push(b as char);
            } else if upper {
                s.push_str(&format!("%{b:02X}"));
            } else {
                s.push_str(&format!("%{b:02x}"));
            }
        }
        s.into_bytes()
    };
    for (name, keep) in [
        ("url", &b"-._~"[..]),
        ("uri-component", b"-._~!*'()"),
        ("path", b"-._~/"),
    ] {
        out.push((format!("{name} upper"), pct(keep, true)));
        out.push((format!("{name} lower"), pct(keep, false)));
    }
    let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
    out.push(("hex".into(), hex.clone().into_bytes()));
    out.push(("HEX".into(), hex.to_uppercase().into_bytes()));
    for (name, engine) in [("base64", &STANDARD), ("base64url", &URL_SAFE)] {
        out.push((name.into(), engine.encode(value).into_bytes()));
    }
    for (name, engine) in [
        ("base64 nopad", &STANDARD_NO_PAD),
        ("base64url nopad", &URL_SAFE_NO_PAD),
    ] {
        for prefix in [&b""[..], b"x", b"xy"] {
            let mut data = prefix.to_vec();
            data.extend_from_slice(value);
            data.extend_from_slice(b"zz");
            let encoded = engine.encode(&data);
            // Well inside the value's span, so these characters depend on it alone.
            let inner = &encoded[4..encoded.len() - 6];
            out.push((
                format!("{name} after {} bytes", prefix.len()),
                inner.as_bytes().to_vec(),
            ));
        }
    }
    out
}

/// The names of the forms of `value` found in `haystack`.
pub fn leaks(haystack: &[u8], value: &[u8]) -> Vec<String> {
    forms(value)
        .into_iter()
        .filter(|(_, form)| memchr::memmem::find(haystack, form).is_some())
        .map(|(name, _)| name)
        .collect()
}

pub fn assert_no_leak(what: &str, haystack: &[u8]) {
    let found = leaks(haystack, VALUE.as_bytes());
    assert!(found.is_empty(), "{what} holds the value as {found:?}");
}

/// Reads a request's body whole, for handlers.
pub async fn body_of(req: Request<Incoming>) -> (http::request::Parts, Vec<u8>) {
    let (parts, body) = req.into_parts();
    (parts, body.collect().await.unwrap().to_bytes().to_vec())
}

pub fn respond(status: u16, body: impl Into<Bytes>) -> Response<TestBody> {
    let mut response = Response::new(full(body));
    *response.status_mut() = http::StatusCode::from_u16(status).unwrap();
    response
}
