//! A request the daemon makes itself, such as the gate's call to a decision model.
//!
//! It takes the proxy's own way out: the host must not be refused, the upstream's certificate is
//! checked against the same trust, `resolve` applies, references are swapped by the same rule,
//! and the answer is redacted before the caller sees it. It differs from a driver's request in
//! three ways. Only the one credential the call names is swapped, and only in its headers: the
//! body may quote an agent's text, and a reference an agent wrote there must stay a reference.
//! The gate is not asked, since the daemon chose the host. And nothing is reported on the
//! proxy's channel: the caller records the call and the credential's use itself.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use tokio_rustls::TlsConnector;
use x100_core::credential::UseOutcome;
use x100_core::id::CredentialRef;

use super::{Proxy, Shared};
use crate::refs::{self, Decide};
use crate::store::Denied;

/// The most of an answer that is read. A decision model's answer is a few hundred bytes.
const MAX_ANSWER: usize = 1 << 20;

/// One HTTPS `POST` the daemon makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonCall {
    pub host: String,
    /// The path and query, such as `/v1/decisions`.
    pub path: String,
    /// The one credential whose reference may be swapped for its value, in the headers only.
    pub credential: CredentialRef,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// For the whole call: connecting, sending and reading the answer.
    pub timeout: Duration,
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
    /// Sends `call` to its host on port 443, over TLS.
    pub async fn call(&self, call: DaemonCall) -> Called {
        let shared = &self.shared;
        if shared.is_refused(&call.host) {
            return Called {
                used: None,
                answer: Err(format!("the proxy refuses {}", call.host)),
            };
        }
        let mut swap = Swap {
            shared: Arc::clone(shared),
            credential: call.credential,
            host: call.host.clone(),
            used: None,
        };
        let request = request(&call, &mut swap);
        let used = swap.used.take();
        drop(swap);
        let answer = match request {
            Ok(request) => {
                match tokio::time::timeout(call.timeout, send(shared, &call.host, request)).await {
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
        HeaderValue::from_str(&call.host).map_err(|_| "a host that is not a header value")?,
    );
    headers.insert(
        header::ACCEPT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(call.body.len()));
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(&call.path)
        .body(Full::new(Bytes::from(call.body.clone())))
        .map_err(|e| format!("the request: {e}"))?;
    *request.headers_mut() = headers;
    Ok(request)
}

async fn send(
    shared: &Shared,
    host: &str,
    request: Request<Full<Bytes>>,
) -> Result<(u16, Vec<u8>), String> {
    let tcp = shared.tcp(host, 443).await?;
    let name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|e| format!("{host}: {e}"))?;
    let tls = TlsConnector::from(Arc::clone(&shared.client))
        .connect(name, tcp)
        .await
        .map_err(|e| format!("TLS to {host}: {e}"))?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
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
    credential: CredentialRef,
    host: String,
    used: Option<UseOutcome>,
}

impl Decide for Swap {
    fn decide(&mut self, reference: CredentialRef, out: &mut Vec<u8>) -> bool {
        if reference != self.credential {
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
