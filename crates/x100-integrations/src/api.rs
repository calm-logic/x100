//! A JSON API the daemon calls through the vault's proxy, with a credential's reference in the
//! `Authorization` header that the proxy swaps for the value. Nothing here handles a value.

use std::time::Duration;

use serde_json::Value;
use x100_core::id::CredentialRef;
use x100_vault::{CallMethod, DaemonCall, Proxy, Scheme};

/// How long one call may take. Every call here is small; a deploy is polled, not waited on.
const WITHIN: Duration = Duration::from_secs(20);

/// Where an API is, and the credential its calls carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub scheme: Scheme,
    pub host: String,
    pub port: Option<u16>,
    pub credential: Option<CredentialRef>,
}

impl Endpoint {
    /// Reads `scheme://host[:port]`, with any path or trailing slash dropped. Plain HTTP is taken
    /// only for this machine's loopback: a key sent elsewhere would cross the network in the
    /// clear.
    pub fn parse(address: &str) -> Result<Endpoint, String> {
        let address = address.trim();
        let (scheme, rest) = if let Some(rest) = address.strip_prefix("https://") {
            (Scheme::Https, rest)
        } else if let Some(rest) = address.strip_prefix("http://") {
            (Scheme::Http, rest)
        } else {
            (Scheme::Https, address)
        };
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.is_empty() || authority.contains('@') {
            return Err(format!("{address:?} is not a host address"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => match p.parse::<u16>() {
                Ok(p) => (h.to_owned(), Some(p)),
                Err(_) => return Err(format!("{address:?} has a port that is not a number")),
            },
            _ => (authority.to_owned(), None),
        };
        let mut endpoint = Endpoint {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            credential: None,
        };
        // An address on this machine given without a scheme is a local Homebox, which serves
        // plain HTTP.
        let bare = !address.contains("://");
        if bare && endpoint.is_loopback() {
            endpoint.scheme = Scheme::Http;
        }
        if scheme == Scheme::Http && !endpoint.is_loopback() {
            return Err(format!(
                "{address} is plain HTTP on another machine, where its key would cross the \
                 network in the clear: give its https address"
            ));
        }
        Ok(endpoint)
    }

    pub fn with(mut self, credential: CredentialRef) -> Endpoint {
        self.credential = Some(credential);
        self
    }

    /// `scheme://host[:port]`, as a connection records it.
    pub fn address(&self) -> String {
        let scheme = match self.scheme {
            Scheme::Https => "https",
            Scheme::Http => "http",
        };
        format!("{scheme}://{}", self.authority())
    }

    /// The host, with the port when it is not the scheme's own: what the key's credential is
    /// scoped to.
    pub fn authority(&self) -> String {
        self.call(CallMethod::Get, "/").authority()
    }

    pub fn is_loopback(&self) -> bool {
        let bare = self.host.trim_start_matches('[').trim_end_matches(']');
        bare == "localhost"
            || bare
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }

    /// The port a plain call to loopback goes to, which the daemon opens on the proxy.
    pub fn loopback_port(&self) -> Option<u16> {
        (self.scheme == Scheme::Http && self.is_loopback()).then(|| self.port.unwrap_or(80))
    }

    fn call(&self, method: CallMethod, path: &str) -> DaemonCall {
        let mut headers = vec![("accept".to_owned(), "application/json".to_owned())];
        if let Some(credential) = self.credential {
            headers.push(("authorization".into(), format!("Bearer {credential}")));
        }
        DaemonCall {
            method,
            scheme: self.scheme,
            host: self.host.clone(),
            port: self.port,
            path: path.to_owned(),
            credential: self.credential,
            headers,
            body: Vec::new(),
            timeout: WITHIN,
        }
    }
}

/// Why a call came to nothing: no answer, or an answer that was not a success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// The status, when there was an answer.
    pub status: Option<u16>,
    pub why: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

pub async fn get(proxy: &Proxy, at: &Endpoint, path: &str) -> Result<Value, ApiError> {
    send(proxy, at.call(CallMethod::Get, path)).await
}

pub async fn post(
    proxy: &Proxy,
    at: &Endpoint,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, ApiError> {
    let mut call = at.call(CallMethod::Post, path);
    if let Some(body) = body {
        call.headers
            .push(("content-type".into(), "application/json".into()));
        call.body = body.to_string().into_bytes();
    }
    send(proxy, call).await
}

/// A POST to a form endpoint, such as GitHub's device flow, which answers JSON when asked to.
pub async fn post_form(
    proxy: &Proxy,
    at: &Endpoint,
    path: &str,
    form: &[(&str, &str)],
) -> Result<Value, ApiError> {
    let mut call = at.call(CallMethod::Post, path);
    call.headers.push((
        "content-type".into(),
        "application/x-www-form-urlencoded".into(),
    ));
    let body: Vec<String> = form
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    call.body = body.join("&").into_bytes();
    send(proxy, call).await
}

async fn send(proxy: &Proxy, call: DaemonCall) -> Result<Value, ApiError> {
    let host = call.authority();
    let path = call.path.clone();
    let called = proxy.call(call).await;
    let (status, body) = called
        .answer
        .map_err(|why| ApiError { status: None, why })?;
    if !(200..300).contains(&status) {
        let said = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("detail").or(v.get("error")).cloned())
            .map(|d| match d {
                Value::String(s) => s,
                other => other.to_string(),
            });
        let why = match said {
            Some(said) => format!("{host} answered {status} to {path}: {}", short(&said)),
            None => format!("{host} answered {status} to {path}"),
        };
        return Err(ApiError {
            status: Some(status),
            why,
        });
    }
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body).map_err(|_| ApiError {
        status: Some(status),
        why: format!("{host} answered {path} with something that is not JSON"),
    })
}

/// A service's own words, on one line and cut, for the builder or an agent to read.
pub fn short(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect()
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_read_and_plain_http_is_for_loopback_alone() {
        let local = Endpoint::parse("http://127.0.0.1:7765/").unwrap();
        assert_eq!(local.address(), "http://127.0.0.1:7765");
        assert_eq!(local.authority(), "127.0.0.1:7765");
        assert_eq!(local.loopback_port(), Some(7765));
        let bare = Endpoint::parse("127.0.0.1:7765").unwrap();
        assert_eq!(bare.address(), "http://127.0.0.1:7765");
        let remote = Endpoint::parse("hb.example.com").unwrap();
        assert_eq!(remote.address(), "https://hb.example.com");
        assert_eq!(remote.loopback_port(), None);
        let port = Endpoint::parse("https://HB.example.com:8443/api").unwrap();
        assert_eq!(port.authority(), "hb.example.com:8443");
        assert!(Endpoint::parse("http://hb.example.com").is_err());
        assert!(Endpoint::parse("https://").is_err());
        assert!(Endpoint::parse("https://hb:x").is_err());
    }
}
