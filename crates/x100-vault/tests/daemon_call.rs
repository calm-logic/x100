//! A request the daemon makes itself through the proxy: the named credential's value goes into
//! its headers for an allowed host and nowhere else, a reference in the body stays a reference,
//! the answer is redacted, a refused host gets nothing, and a host that never answers is given up
//! on.

mod common;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use x100_core::credential::{Credential, Projects, Refusal, UseOutcome};
use x100_core::id::CredentialRef;
use x100_vault::{DaemonCall, NoKeychain, Proxy, ProxyConfig, Resolved, SecretValue, Vault};

const DECIDER: &str = "decide.x100.test";
const ELSEWHERE: &str = "elsewhere.x100.test";

struct Rig {
    _dir: tempfile::TempDir,
    proxy: Proxy,
    reference: CredentialRef,
}

/// A vault holding one key for `DECIDER` that no project may use, and a closed proxy that sends
/// both test hosts to `port` on loopback.
fn rig(ca: &TestCa, port: u16) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("vault"), &NoKeychain).unwrap();
    let reference = CredentialRef::from_u128(0x0de1);
    let credential = Credential {
        reference,
        name: "decider".into(),
        scope: scope(&[DECIDER], Projects::Only(Default::default())),
    };
    vault.add(credential, SecretValue::from(VALUE)).unwrap();
    let at = Resolved {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: Some(port),
    };
    let config = ProxyConfig {
        upstream: x100_vault::UpstreamTrust::Only(vec![ca.cert.clone()]),
        resolve: HashMap::from([(DECIDER.to_owned(), at), (ELSEWHERE.to_owned(), at)]),
        closed: true,
    };
    let authority = vault.authority().unwrap();
    let (proxy, _reports) = Proxy::new(vault, authority, config).unwrap();
    Rig {
        _dir: dir,
        proxy,
        reference,
    }
}

type Got = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Echoes the authorization it was sent, and the body.
async fn echo(ca: &TestCa) -> (TestServer, Got) {
    let got: Got = Arc::default();
    let log = Arc::clone(&got);
    let server = TestServer::https(Some(ca.server_config(&[DECIDER, ELSEWHERE])), move |req| {
        let log = Arc::clone(&log);
        async move {
            let (parts, body) = body_of(req).await;
            let auth = parts
                .headers
                .get("authorization")
                .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
                .unwrap_or_default();
            log.lock().unwrap().push((auth.clone(), body.clone()));
            let mut out = format!("you sent {auth} and ").into_bytes();
            out.extend(body);
            respond(200, out)
        }
    })
    .await;
    (server, got)
}

fn call(rig: &Rig, host: &str) -> DaemonCall {
    DaemonCall {
        host: host.into(),
        path: "/v1/decisions".into(),
        credential: rig.reference,
        headers: vec![
            ("authorization".into(), format!("Bearer {}", rig.reference)),
            ("content-type".into(), "application/json".into()),
        ],
        body: format!("{{\"input\":\"an agent wrote {}\"}}", rig.reference).into_bytes(),
        timeout: Duration::from_secs(10),
    }
}

#[tokio::test]
async fn the_key_goes_into_the_headers_for_its_host_and_the_answer_is_redacted() {
    let ca = TestCa::new();
    let (server, got) = echo(&ca).await;
    let rig = rig(&ca, server.port);
    let called = rig.proxy.call(call(&rig, DECIDER)).await;
    assert_eq!(called.used, Some(UseOutcome::Injected));
    let (status, body) = called.answer.expect("an answer");
    assert_eq!(status, 200);
    let (auth, sent) = got.lock().unwrap()[0].clone();
    assert_eq!(auth, format!("Bearer {VALUE}"), "the host got the value");
    let sent = String::from_utf8(sent).unwrap();
    assert!(
        sent.contains(&rig.reference.to_string()),
        "a reference in the body stays one: {sent}"
    );
    assert_no_leak("the answer", &body);
    assert!(String::from_utf8_lossy(&body).contains(&rig.reference.to_string()));
}

#[tokio::test]
async fn another_host_gets_the_reference_and_the_refusal_is_said() {
    let ca = TestCa::new();
    let (server, got) = echo(&ca).await;
    let rig = rig(&ca, server.port);
    let called = rig.proxy.call(call(&rig, ELSEWHERE)).await;
    assert_eq!(
        called.used,
        Some(UseOutcome::Refused(Refusal::HostNotAllowed))
    );
    assert_eq!(called.answer.unwrap().0, 200);
    assert_eq!(
        got.lock().unwrap()[0].0,
        format!("Bearer {}", rig.reference)
    );
}

#[tokio::test]
async fn a_host_the_proxy_refuses_is_not_reached() {
    let ca = TestCa::new();
    let (server, got) = echo(&ca).await;
    let rig = rig(&ca, server.port);
    let called = rig.proxy.call(call(&rig, "not-resolved.x100.test")).await;
    assert_eq!(called.used, None);
    assert!(called.answer.unwrap_err().contains("refuses"));
    assert!(got.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_host_that_never_answers_is_given_up_on() {
    let ca = TestCa::new();
    let server = TestServer::https(Some(ca.server_config(&[DECIDER])), |_| async {
        tokio::time::sleep(Duration::from_secs(60)).await;
        respond(200, "late")
    })
    .await;
    let rig = rig(&ca, server.port);
    let mut slow = call(&rig, DECIDER);
    slow.timeout = Duration::from_millis(300);
    let why = rig.proxy.call(slow).await.answer.unwrap_err();
    assert!(why.contains("did not answer"), "{why}");
}
