//! The egress proxy: the only route out of a sandbox.
//!
//! Each driver gets a [`Route`]: a listener of its own on loopback, and a token it must present
//! as `Proxy-Authorization`. Both identify the driver. The sandbox backend maps only that
//! listener into the sandbox, and the token, which sits in that sandbox's environment alone,
//! stops a container that can reach other host ports from passing as another driver.
//!
//! Plain HTTP arrives as absolute-form requests. HTTPS arrives as `CONNECT`, and the proxy ends
//! TLS itself with a leaf certificate for the host, minted by the [`Authority`], offering only
//! HTTP/1.1. Every request is then put to the [`Admission`] the daemon gives (the gate), and
//! only if it is admitted forwarded, with references swapped for values where the credential
//! allows it, and its response redacted on the way back. A request to the driver's own model
//! provider is not put to it: x100 chose that host, not the agent.

mod body;
mod call;
mod forward;
pub(crate) mod tls;

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use x100_core::action::Request as Action;
use x100_core::credential::{CredentialUse, UseOutcome, Via};
use x100_core::id::{ActionId, CredentialRef};

use crate::scope::Requester;
use crate::{Vault, VaultError};
pub use call::{Called, DaemonCall};
pub use tls::{Authority, UpstreamTrust};

/// Where a host in [`ProxyConfig::resolve`] is sent instead of being looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub ip: IpAddr,
    /// The port to connect to instead of the one asked for. A request the daemon makes itself
    /// always asks for 443, which a test's server on loopback cannot listen on.
    pub port: Option<u16>,
}

impl From<IpAddr> for Resolved {
    fn from(ip: IpAddr) -> Self {
        Self { ip, port: None }
    }
}

pub struct ProxyConfig {
    /// Which authorities the real upstream's certificate must chain to.
    pub upstream: UpstreamTrust,
    /// Hosts sent to a fixed address instead of being looked up, as curl's `--resolve` does.
    /// For tests.
    pub resolve: HashMap<String, Resolved>,
    /// Only the hosts in `resolve` are reached; every other is refused. For tests, so that
    /// nothing a program tries leaves the machine.
    pub closed: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            upstream: UpstreamTrust::System,
            resolve: HashMap::new(),
            closed: false,
        }
    }
}

/// What the gate says about one request, asked before anything of it goes out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Send it. The gate recorded it as `action`, which its report names.
    Go { action: ActionId },
    /// Answer it at once with 403 and `why`, which the program that sent it reads. The gate
    /// recorded it as `action`, if it recorded it at all.
    Refuse {
        action: Option<ActionId>,
        why: String,
    },
}

/// Decides each request a driver sends out, other than to its own model provider. It may take
/// its time: the request waits, and nothing of it has gone out.
#[async_trait]
pub trait Admission: Send + Sync {
    async fn admit(&self, requester: Requester, method: &str, host: &str) -> Admit;
}

/// One request through the proxy, told to the caller once the request has gone out (or was
/// refused or failed), so the wiring can record credential uses and how the gate's action ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressReport {
    pub requester: Requester,
    pub method: String,
    pub host: String,
    pub port: u16,
    /// The gate's action for it. `None` when the gate was not asked: a request to the driver's
    /// own provider, one to a host the proxy refuses, or one that broke before it was asked.
    pub action: Option<ActionId>,
    /// One entry per credential and outcome, in the order first seen.
    pub uses: Vec<(CredentialRef, UseOutcome)>,
    pub outcome: EgressOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressOutcome {
    /// The upstream answered with this status.
    Answered(u16),
    /// The host is refused; nothing went out.
    HostRefused,
    /// The gate did not admit it; nothing went out.
    NotAdmitted,
    /// A tunnel to a pass-through host was opened; what went through it is not known.
    Tunnelled,
    /// The request could not be completed: no connection, a certificate that did not check out,
    /// or a broken stream.
    Failed(String),
}

impl EgressReport {
    /// The `CredentialUse` events this request makes.
    pub fn credential_uses(&self) -> Vec<CredentialUse> {
        self.uses
            .iter()
            .map(|(credential, outcome)| CredentialUse {
                credential: *credential,
                agent: self.requester.agent,
                driver: self.requester.driver,
                via: Via::Proxy {
                    host: self.host.clone(),
                },
                outcome: outcome.clone(),
            })
            .collect()
    }

    /// The request as the gate's action asks it.
    pub fn request(&self) -> Action {
        Action::Egress {
            driver: self.requester.driver,
            host: self.host.clone(),
            method: self.method.clone(),
        }
    }
}

pub(crate) struct Shared {
    vault: Vault,
    authority: Authority,
    client: Arc<rustls::ClientConfig>,
    resolve: HashMap<String, Resolved>,
    closed: bool,
    refused: RwLock<BTreeSet<String>>,
    reports: mpsc::UnboundedSender<EgressReport>,
    /// Set once, by the daemon, after the proxy is made. Until then every request is refused.
    admission: OnceLock<Arc<dyn Admission>>,
}

impl Shared {
    /// A host refused by name, any host outside `resolve` when the proxy is closed, and the
    /// host's own loopback, which holds the daemon and whatever else the builder serves locally:
    /// a sandbox reaches what runs inside it directly, and nothing on the host's loopback
    /// through the proxy. A name that only resolves to loopback is not caught here.
    fn is_refused(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if self.resolve.contains_key(&host) {
            return self.refused.read().expect("refused hosts").contains(&host);
        }
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let loopback = bare == "localhost"
            || bare.ends_with(".localhost")
            || bare
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified());
        self.closed || loopback || self.refused.read().expect("refused hosts").contains(&host)
    }

    /// A connection to `host`, at the address `resolve` gives it if it gives one.
    async fn tcp(&self, host: &str, port: u16) -> Result<tokio::net::TcpStream, String> {
        use tokio::net::TcpStream;
        match self.resolve.get(&host.to_ascii_lowercase()) {
            Some(r) => TcpStream::connect((r.ip, r.port.unwrap_or(port))).await,
            None => TcpStream::connect((host, port)).await,
        }
        .map_err(|e| format!("connecting to {host}:{port}: {e}"))
    }
}

/// The proxy. Cheap to clone. Reports arrive on the receiver [`Proxy::new`] returns, which the
/// caller must keep draining.
#[derive(Clone)]
pub struct Proxy {
    shared: Arc<Shared>,
}

impl Proxy {
    pub fn new(
        vault: Vault,
        authority: Authority,
        config: ProxyConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<EgressReport>), VaultError> {
        let (reports, receiver) = mpsc::unbounded_channel();
        let client = tls::client_config(&config.upstream)?;
        let shared = Shared {
            vault,
            authority,
            client,
            resolve: config
                .resolve
                .into_iter()
                .map(|(h, ip)| (h.to_ascii_lowercase(), ip))
                .collect(),
            closed: config.closed,
            refused: RwLock::new(BTreeSet::new()),
            reports,
            admission: OnceLock::new(),
        };
        Ok((
            Self {
                shared: Arc::new(shared),
            },
            receiver,
        ))
    }

    pub fn authority(&self) -> &Authority {
        &self.shared.authority
    }

    /// Puts every later request to `admission`. Set once; a second call changes nothing. Until
    /// it is set every request is refused, so nothing goes out that the gate did not see.
    pub fn admit_with(&self, admission: Arc<dyn Admission>) {
        let _ = self.shared.admission.set(admission);
    }

    /// Opens a route for one driver: a fresh listener on loopback and a fresh token. Dropping
    /// the route closes the listener and every connection made through it.
    ///
    /// Tunnels to a host in `pass_through` are relayed as bytes, with TLS end to end between
    /// the driver and the host: nothing is swapped, redacted or seen. That is for a terminal
    /// driver's provider, whose traffic carries the builder's login, which x100 must not see.
    /// Requests to a host in `provider`, the model provider x100 started the driver on, are
    /// swapped and redacted as usual but not put to the admission.
    pub async fn open_route(
        &self,
        requester: Requester,
        pass_through: &[&str],
        provider: &[&str],
    ) -> io::Result<Route> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let mut token = [0u8; 16];
        crate::seal::random(&mut token).map_err(io::Error::other)?;
        let token = crate::seal::hex(&token);
        let (close, closed) = watch::channel(());
        let ctx = Arc::new(forward::RouteCtx {
            shared: Arc::clone(&self.shared),
            requester,
            token: token.clone(),
            pass_through: pass_through
                .iter()
                .map(|h| h.to_ascii_lowercase())
                .collect(),
            provider: provider.iter().map(|h| h.to_ascii_lowercase()).collect(),
            closed,
        });
        let task = tokio::spawn(forward::accept(listener, ctx));
        Ok(Route {
            addr,
            token,
            task,
            _close: close,
        })
    }

    /// From now on, requests to `host` are refused with 403, and so are tunnels to it.
    pub fn refuse_host(&self, host: &str) {
        self.shared
            .refused
            .write()
            .expect("refused hosts")
            .insert(host.to_ascii_lowercase());
    }

    pub fn allow_host(&self, host: &str) {
        self.shared
            .refused
            .write()
            .expect("refused hosts")
            .remove(&host.to_ascii_lowercase());
    }
}

/// One driver's way out.
pub struct Route {
    addr: SocketAddr,
    token: String,
    task: JoinHandle<()>,
    /// Dropped with the route, which ends every connection's task.
    _close: watch::Sender<()>,
}

impl Route {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The proxy URL with this route's token, for a client on the daemon's side.
    pub fn url(&self) -> String {
        self.url_at(&self.addr.to_string())
    }

    /// The proxy URL as seen from inside a sandbox, where a forwarder makes the route reachable
    /// at `addr`.
    pub fn url_at(&self, addr: &str) -> String {
        format!("http://x100:{}@{addr}", self.token)
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The environment that sends a sandbox's traffic through `proxy_url` and makes its programs
/// trust the authority at `authority` (paths as seen inside the sandbox). `no_proxy` lists the
/// addresses that must not go through the proxy, such as the daemon's tool address.
pub fn sandbox_env(proxy_url: &str, authority: &Path, no_proxy: &[&str]) -> Vec<(String, String)> {
    let cert = authority.display().to_string();
    let mut env = Vec::new();
    for name in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        env.push((name.to_owned(), proxy_url.to_owned()));
    }
    let no_proxy = no_proxy.join(",");
    env.push(("NO_PROXY".to_owned(), no_proxy.clone()));
    env.push(("no_proxy".to_owned(), no_proxy));
    // Node (Claude Code) adds to its trust; Codex adds to its trust; the rest replace the trust
    // store, which is right inside a sandbox since every TLS connection ends at the proxy.
    for name in [
        "NODE_EXTRA_CA_CERTS",
        "CODEX_CA_CERTIFICATE",
        "SSL_CERT_FILE",
        "CURL_CA_BUNDLE",
        "REQUESTS_CA_BUNDLE",
        "GIT_SSL_CAINFO",
    ] {
        env.push((name.to_owned(), cert.clone()));
    }
    env
}
