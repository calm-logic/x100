//! A request the daemon makes itself, such as the gate's call to a decision model or a call to an
//! integration's API.
//!
//! It takes the proxy's own way out: the host must not be refused, the upstream's certificate is
//! checked against the same trust, `resolve` applies, references are swapped by the same rule,
//! and the answer is redacted before the caller sees it. It differs from a driver's request in
//! three ways. Only the one credential the call names is swapped, and only in its headers: the
//! body may quote an agent's text, and a reference an agent wrote there must stay a reference.
//! The gate is not asked, since the daemon chose the host. And nothing is reported on the
//! proxy's channel: the caller records the call and the credential's use itself.
//!
//! Plain HTTP goes only to a port on the host's loopback that the daemon opened for its own calls
//! with [`Proxy::open_loopback`], such as a Homebox on this machine. Loopback stays refused to
//! every driver's route whatever is opened here: that check is in `forward` and does not read
//! this list.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsConnector;
use x100_core::credential::UseOutcome;
use x100_core::id::CredentialRef;

use super::{Proxy, Shared};
use crate::refs::{self, Decide};
use crate::store::Denied;

/// The most of an answer that is read. A decision model's answer is a few hundred bytes, and an
/// integration's list of projects a few hundred kilobytes at most.
const MAX_ANSWER: usize = 4 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Https,
    /// Only to a loopback port opened with [`Proxy::open_loopback`].
    Http,
}

/// One request the daemon makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonCall {
    pub method: CallMethod,
    pub scheme: Scheme,
    pub host: String,
    /// `None` for the scheme's own: 443 or 80.
    pub port: Option<u16>,
    /// The path and query, such as `/v1/decisions`.
    pub path: String,
    /// The one credential whose reference may be swapped for its value, in the headers only.
    pub credential: Option<CredentialRef>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// For the whole call: connecting, sending and reading the answer.
    pub timeout: Duration,
}

impl DaemonCall {
    /// The host, with the port when it is not the scheme's own: what a credential's scope names
    /// and what the `Host` header carries. A key for a Homebox on `127.0.0.1:7765` reaches no
    /// other service on loopback.
    pub fn authority(&self) -> String {
        let default = match self.scheme {
            Scheme::Https => 443,
            Scheme::Http => 80,
        };
        match self.port {
            Some(p) if p != default => format!("{}:{p}", self.host),
            _ => self.host.clone(),
        }
    }
}

/// What came of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Called {
    /// What became of the credential: put in, or refused because its scope does not allow the
    /// host. `None` when its reference was not in the headers, or the credential is unknown.
    pub used: Option<UseOutcome>,
    /// The answer's status and its body, redacted of every value the vault knows; or why there
    /// was no answer.
    pub answer: Result<(u16, Vec<u8>), String>,
}

impl Proxy {
    /// Sends `call` to its host: over TLS, or as plain HTTP to an opened loopback port.
    pub async fn call(&self, call: DaemonCall) -> Called {
        let shared = &self.shared;
        let refused = |why: String| Called {
            used: None,
            answer: Err(why),
        };
        let plain_to = match call.scheme {
            Scheme::Https if shared.is_refused(&call.host) => {
                return refused(format!("the proxy refuses {}", call.host));
            }
            Scheme::Https => None,
            Scheme::Http => match self.opened(&call) {
                Some(ip) => Some(ip),
                None => {
                    return refused(format!(
                        "plain HTTP goes only to a loopback port opened for the daemon, and {} is not one",
                        call.authority()
                    ));
                }
            },
        };
        let mut swap = Swap {
            shared: Arc::clone(shared),
            credential: call.credential,
            host: call.authority(),
            used: None,
        };
        let request = request(&call, &mut swap);
        let used = swap.used.take();
        drop(swap);
        let answer = match request {
            Ok(request) => {
                let sent = async {
                    match plain_to {
                        Some(ip) => {
                            let port = call.port.unwrap_or(80);
                            let tcp = tokio::net::TcpStream::connect((ip, port))
                                .await
                                .map_err(|e| format!("connecting to {}: {e}", call.authority()))?;
                            exchange(tcp, &call.host, request).await
                        }
                        None => send_tls(shared, &call, request).await,
                    }
                };
                match tokio::time::timeout(call.timeout, sent).await {
                    Ok(answer) => answer,
                    Err(_) => Err(format!(
                        "{} did not answer within {} seconds",
                        call.host,
                        call.timeout.as_secs()
                    )),
                }
            }
            Err(e) => Err(e),
        };
        let redactor = shared.vault.redactor();
        Called {
            used,
            answer: answer.map(|(status, body)| (status, redactor.redact(&body))),
        }
    }

    /// From now on the daemon's own calls may send plain HTTP to `port` on this machine's
    /// loopback. No driver's route is affected.
    pub fn open_loopback(&self, port: u16) {
        self.shared
            .daemon_loopback
            .write()
            .expect("opened ports")
            .insert(port);
    }

    pub fn close_loopback(&self, port: u16) {
        self.shared
            .daemon_loopback
            .write()
            .expect("opened ports")
            .remove(&port);
    }

    /// The loopback address a plain call goes to, if its host is loopback and its port opened.
    fn opened(&self, call: &DaemonCall) -> Option<IpAddr> {
        let bare = call.host.trim_start_matches('[').trim_end_matches(']');
        let ip = match bare {
            "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
            other => other.parse::<IpAddr>().ok()?,
        };
        let loopback =
            ip == IpAddr::V4(Ipv4Addr::LOCALHOST) || ip == IpAddr::V6(Ipv6Addr::LOCALHOST);
        let port = call.port.unwrap_or(80);
        let open = self
            .shared
            .daemon_loopback
            .read()
            .expect("opened ports")
            .contains(&port);
        (loopback && open).then_some(ip)
    }
}

fn request(call: &DaemonCall, swap: &mut Swap) -> Result<Request<Full<Bytes>>, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in &call.headers {
        let name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|e| format!("header {name:?}: {e}"))?;
        let swapped = refs::swap_all(value.as_bytes(), swap);
        let value = HeaderValue::from_bytes(&swapped)
            .map_err(|_| format!("header {name} is not a header value"))?;
        headers.append(name, value);
    }
    super::forward::strip_hop_by_hop(&mut headers);
    headers.insert(
        header::HOST,
        HeaderValue::from_str(&call.authority())
            .map_err(|_| "a host that is not a header value")?,
    );
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    let method = match call.method {
        CallMethod::Get => Method::GET,
        CallMethod::Post => Method::POST,
    };
    if call.method == CallMethod::Post || !call.body.is_empty() {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(call.body.len()));
    }
    let mut request = Request::builder()
        .method(method)
        .uri(&call.path)
        .body(Full::new(Bytes::from(call.body.clone())))
        .map_err(|e| format!("the request: {e}"))?;
    *request.headers_mut() = headers;
    Ok(request)
}

async fn send_tls(
    shared: &Shared,
    call: &DaemonCall,
    request: Request<Full<Bytes>>,
) -> Result<(u16, Vec<u8>), String> {
    let host = &call.host;
    let tcp = shared.tcp(host, call.port.unwrap_or(443)).await?;
    let name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|e| format!("{host}: {e}"))?;
    let tls = TlsConnector::from(Arc::clone(&shared.client))
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS to {host}: {e}"))?;
    exchange(tls, host, request).await
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    host: &str,
    request: Request<Full<Bytes>>,
) -> Result<(u16, Vec<u8>), String> {
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("{host}: {e}"))?;
    // Ends when the connection does, which is when this call is done with it.
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| format!("{host} gave no answer: {e}"))?;
    let status = response.status().as_u16();
    let body = Limited::new(response.into_body(), MAX_ANSWER)
        .collect()
        .await
        .map_err(|e| format!("reading {host}'s answer: {e}"))?
        .to_bytes();
    Ok((status, body.to_vec()))
}

/// Swaps the call's own credential, if its scope allows the host, and leaves every other
/// reference as it is.
struct Swap {
    shared: Arc<Shared>,
    credential: Option<CredentialRef>,
    /// The call's authority: the host, with its port when not the scheme's own.
    host: String,
    used: Option<UseOutcome>,
}

impl Decide for Swap {
    fn decide(&mut self, reference: CredentialRef, out: &mut Vec<u8>) -> bool {
        if Some(reference) != self.credential {
            return false;
        }
        match self.shared.vault.take_for_daemon(reference, &self.host) {
            Ok(value) => {
                out.extend_from_slice(value.expose());
                self.used = Some(UseOutcome::Injected);
                true
            }
            Err(Denied::Refused(refusal)) => {
                self.used = Some(UseOutcome::Refused(refusal));
                false
            }
            Err(Denied::Unknown) => false,
        }
    }
}
