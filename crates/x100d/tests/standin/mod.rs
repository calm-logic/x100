//! A stand-in for the three decision models' APIs, on one HTTPS port on loopback with a
//! certificate for all three hosts from a test authority, and a plain HTTP server that stands in
//! for the hosts agents send to.
//!
//! The stand-in checks that each request is the one its provider documents (the path, the pinned
//! model, two choice questions and the input) and answers in that provider's shape with what the
//! test told it to say. An error it gives quotes the key it was sent, as some providers do, so a
//! test can see that the daemon redacts it.

#![allow(dead_code)]

pub mod homebox;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use x100_core::decision::DecisionProvider;

/// What every answer says the input came to.
pub const INPUT_TOKENS: u64 = 2_000;

/// What the stand-in answers next.
#[derive(Debug, Clone)]
pub enum Says {
    Labels {
        kind: &'static str,
        risk: &'static str,
        confidence: f64,
    },
    /// An error, in the provider's own shape, quoting the key.
    Fail(u16),
    /// The provider declines: OpenAI's refusal of a question, Gemini's blocked input. Jev
    /// documents none, so it answers only one of the two questions.
    Refuse,
    /// Closes the connection without a word.
    HangUp,
}

/// One request the stand-in was sent.
#[derive(Debug, Clone)]
pub struct Received {
    pub host: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

pub struct StandIn {
    pub port: u16,
    /// The test authority's certificate, which the daemon is told to trust upstream.
    pub ca: PathBuf,
    says: Arc<Mutex<Says>>,
    received: Arc<Mutex<Vec<Received>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn provider_of(host: &str) -> Option<DecisionProvider> {
    DecisionProvider::ALL.into_iter().find(|p| p.host() == host)
}

/// A TLS acceptor with one certificate for `hosts`, signed by a new test authority whose
/// certificate is written to `ca` for the daemon to trust upstream.
pub fn tls(ca: &Path, hosts: Vec<String>) -> tokio_rustls::TlsAcceptor {
    let ca_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_cert = params.self_signed(&ca_key).unwrap();
    std::fs::write(ca, ca_cert.pem()).unwrap();
    let issuer = Issuer::new(params, ca_key);
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(hosts)
        .unwrap()
        .signed_by(&leaf_key, &issuer)
        .unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let chain: Vec<CertificateDer<'static>> = vec![leaf.der().clone()];
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .unwrap();
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

impl StandIn {
    pub async fn start(dir: &Path) -> StandIn {
        let ca = dir.join("standin-ca.pem");
        let hosts: Vec<String> = DecisionProvider::ALL
            .iter()
            .map(|p| p.host().to_owned())
            .collect();
        let acceptor = tls(&ca, hosts);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let says = Arc::new(Mutex::new(Says::HangUp));
        let received: Arc<Mutex<Vec<Received>>> = Arc::default();
        let (s, r) = (Arc::clone(&says), Arc::clone(&received));
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, s, r) = (acceptor.clone(), Arc::clone(&s), Arc::clone(&r));
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        serve(tls, &s, &r).await;
                    }
                });
            }
        });
        StandIn {
            port,
            ca,
            says,
            received,
            task,
        }
    }

    pub fn say(&self, says: Says) {
        *self.says.lock().unwrap() = says;
    }

    pub fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    says: &Mutex<Says>,
    received: &Mutex<Vec<Received>>,
) {
    let Some((path, headers, body)) = read_request(&mut s).await else {
        return;
    };
    let host = headers.get("host").cloned().unwrap_or_default();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    received.lock().unwrap().push(Received {
        host: host.clone(),
        path: path.clone(),
        headers: headers.clone(),
        body: body.clone(),
    });
    let Some(provider) = provider_of(&host) else {
        return reply(&mut s, 404, &json!({"detail": "no such host"})).await;
    };
    if let Err(why) = documented(provider, &path, &body) {
        return reply(&mut s, 400, &json!({"detail": why})).await;
    }
    let key = match provider {
        DecisionProvider::Gemini => headers.get("x-goog-api-key").cloned(),
        _ => headers
            .get("authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .map(str::to_owned),
    }
    .unwrap_or_default();
    let says = says.lock().unwrap().clone();
    match says {
        Says::HangUp => {
            let _ = s.shutdown().await;
        }
        Says::Fail(status) => reply(&mut s, status, &failure(provider, status, &key)).await,
        Says::Refuse => reply(&mut s, 200, &refusal(provider)).await,
        Says::Labels {
            kind,
            risk,
            confidence,
        } => reply(&mut s, 200, &answer(provider, kind, risk, confidence)).await,
    }
}

/// The path, the pinned model, the input, and the two questions, each in its provider's shape.
fn documented(provider: DecisionProvider, path: &str, body: &Value) -> Result<(), String> {
    let (want_path, model, input) = match provider {
        DecisionProvider::Gemini => ("/v1beta/decisions", "gemini-decision-1", "input"),
        DecisionProvider::OpenAi => ("/v1/decisions", "gpt-6-luna", "input"),
        DecisionProvider::Jev => ("/v1/systemone", "jev-1.14.0", "state"),
    };
    if path != want_path {
        return Err(format!("no such path {path}"));
    }
    if body["model"] != model {
        return Err(format!("unknown model {}", body["model"]));
    }
    if !body[input]
        .as_str()
        .is_some_and(|i| i.starts_with("UNTRUSTED"))
    {
        return Err("no input".into());
    }
    let questions: Vec<&Value> = match provider {
        DecisionProvider::OpenAi => body["questions"].as_array().map(|a| a.iter().collect()),
        _ => body["questions"].as_object().map(|m| m.values().collect()),
    }
    .ok_or("no questions")?;
    let choices = questions.iter().all(|q| {
        q["type"] == "choice"
            && q["instructions"].as_str().is_some()
            && (q["criteria"].is_object() || q["choices"].is_array())
    });
    if questions.len() != 2 || !choices {
        return Err("the questions are not two choices".into());
    }
    Ok(())
}

fn choice(choice: &str, confidence: f64) -> Value {
    json!({"type": "choice", "choice": choice, "confidence": confidence})
}

fn answer(provider: DecisionProvider, kind: &str, risk: &str, confidence: f64) -> Value {
    match provider {
        DecisionProvider::Gemini => json!({
            "answers": {"kind": choice(kind, 0.9), "risk": choice(risk, confidence)},
            "usage": {"total_input_tokens": INPUT_TOKENS, "total_output_tokens": 0},
        }),
        DecisionProvider::OpenAi => {
            let mut k = choice(kind, 0.9);
            k["name"] = "kind".into();
            let mut r = choice(risk, confidence);
            r["name"] = "risk".into();
            json!({"model": "gpt-6-luna", "answers": [k, r],
                   "usage": {"input_tokens": INPUT_TOKENS, "output_tokens": 0}})
        }
        DecisionProvider::Jev => json!({
            "model": "jev-1.14.0",
            "answers": {"kind": choice(kind, 0.9), "risk": choice(risk, confidence)},
            "usage": {"input_tokens": INPUT_TOKENS, "output_tokens": 3},
        }),
    }
}

fn refusal(provider: DecisionProvider) -> Value {
    match provider {
        DecisionProvider::Gemini => json!({
            "answers": {},
            "prompt_feedback": {"block_reason": "safety"},
            "usage": {"total_input_tokens": INPUT_TOKENS},
        }),
        DecisionProvider::OpenAi => {
            let mut k = choice("none_of_these", 0.9);
            k["name"] = "kind".into();
            json!({"answers": [k, {"type": "refusal", "name": "risk"}],
                   "usage": {"input_tokens": INPUT_TOKENS}})
        }
        DecisionProvider::Jev => json!({
            "answers": {"kind": choice("none_of_these", 0.9)},
            "usage": {"input_tokens": INPUT_TOKENS},
        }),
    }
}

fn failure(provider: DecisionProvider, status: u16, key: &str) -> Value {
    match provider {
        DecisionProvider::Gemini => json!({"error": {
            "code": status, "message": format!("API key not valid: {key}"), "status": "INVALID_ARGUMENT",
        }}),
        DecisionProvider::OpenAi => json!({"error": {
            "message": format!("Incorrect API key provided: {key}"), "type": "invalid_request_error",
        }}),
        DecisionProvider::Jev => json!({"detail": format!("Invalid API key {key}")}),
    }
}

async fn reply<S: AsyncWrite + Unpin>(s: &mut S, status: u16, body: &Value) {
    let body = body.to_string();
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes()).await;
    let _ = s.write_all(body.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// The path, headers with lower-case names, and body of one request.
async fn read_request<S: AsyncRead + Unpin>(
    s: &mut S,
) -> Option<(String, HashMap<String, String>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.lines();
    let path = lines.next()?.split_whitespace().nth(1)?.to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end..].to_vec();
    while body.len() < length {
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((path, headers, body))
}

/// Counts the requests each host was sent over plain HTTP, and answers every one with 200.
pub type Seen = Arc<Mutex<HashMap<String, usize>>>;

pub async fn plain_server() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Some((_, headers, _)) = read_request(&mut s).await else {
                    return;
                };
                let host = headers.get("host").cloned().unwrap_or_default();
                let host = host.split(':').next().unwrap_or_default().to_owned();
                *log.lock().unwrap().entry(host).or_default() += 1;
                let reply = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, seen)
}
