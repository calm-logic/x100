//! The egress proxy against local HTTPS servers. The first test is the plan's M11 check as far as
//! this crate reaches.

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use common::*;
use http::Request;
use x100_core::credential::{Projects, Refusal, UseOutcome};
use x100_core::driver::Purpose;
use x100_core::id::ProjectId;
use x100_core::id::TrialId;
use x100_core::trial::Arm;
use x100_vault::{EgressOutcome, UpstreamTrust};

type Log = Arc<Mutex<Vec<String>>>;

/// A server that answers only to the real value and records the authorisation it was given.
async fn guarded(ca: &TestCa, host: &str, log: Log) -> TestServer {
    TestServer::https(
        Some(ca.server_config(&[host])),
        move |req: Request<hyper::body::Incoming>| {
            let log = Arc::clone(&log);
            async move {
                let auth = req
                    .headers()
                    .get("authorization")
                    .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
                    .unwrap_or_default();
                log.lock().unwrap().push(auth.clone());
                if auth == format!("Bearer {VALUE}") {
                    respond(200, "deployed")
                } else {
                    respond(401, "who are you")
                }
            }
        },
    )
    .await
}

fn bearer(reference: impl std::fmt::Display) -> Request<TestBody> {
    Request::post("/deploy")
        .header("host", ALLOWED)
        .header("authorization", format!("Bearer {reference}"))
        .body(full("{\"app\":\"site\"}"))
        .unwrap()
}

#[tokio::test]
async fn an_authenticated_request_with_only_a_reference_succeeds_and_the_value_is_seen_nowhere() {
    let mut s = setup();
    let allowed_log = Log::default();
    let other_log = Log::default();
    let allowed = guarded(&s.ca, ALLOWED, Arc::clone(&allowed_log)).await;
    let other = guarded(&s.ca, OTHER, Arc::clone(&other_log)).await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let mut seen_by_client: Vec<u8> = Vec::new();

    // The client knows only the reference, and succeeds.
    let request = bearer(s.reference);
    seen_by_client.extend(format!("{:?}", request).into_bytes());
    let ok = https_via(&route, &s.authority, ALLOWED, allowed.port, request).await;
    assert_eq!((ok.status, ok.text().as_str()), (200, "deployed"));
    seen_by_client.extend(ok.all());
    assert_eq!(
        *allowed_log.lock().unwrap(),
        vec![format!("Bearer {VALUE}")]
    );
    let report = s.report().await;
    assert_eq!(report.host, ALLOWED);
    assert_eq!(report.method, "POST");
    assert_eq!(report.uses, vec![(s.reference, UseOutcome::Injected)]);
    assert_eq!(report.outcome, EgressOutcome::Answered(200));
    seen_by_client.extend(
        format!(
            "{report:?} {:?} {:?}",
            report.credential_uses(),
            report.action()
        )
        .into_bytes(),
    );

    // The same request to a host the credential is not allowed on goes out without the value.
    let mut request = bearer(s.reference);
    request.headers_mut().insert("host", OTHER.parse().unwrap());
    seen_by_client.extend(format!("{:?}", request).into_bytes());
    let refused = https_via(&route, &s.authority, OTHER, other.port, request).await;
    assert_eq!(refused.status, 401);
    seen_by_client.extend(refused.all());
    assert_eq!(
        *other_log.lock().unwrap(),
        vec![format!("Bearer {}", s.reference)]
    );
    let report = s.report().await;
    assert_eq!(report.host, OTHER);
    assert_eq!(
        report.uses,
        vec![(s.reference, UseOutcome::Refused(Refusal::HostNotAllowed))]
    );
    seen_by_client.extend(format!("{report:?} {:?}", report.credential_uses()).into_bytes());

    assert_no_leak("what the client side saw", &seen_by_client);
    // The search itself finds the value when it is there.
    assert_eq!(
        leaks(format!("x{VALUE}y").as_bytes(), VALUE.as_bytes()),
        vec!["raw"]
    );
}

#[tokio::test]
async fn a_server_that_echoes_the_value_back_is_redacted_on_the_way_to_the_client() {
    let mut s = setup();
    let echo = TestServer::https(Some(s.ca.server_config(&[ALLOWED])), |req| async move {
        let (parts, body) = body_of(req).await;
        let auth = parts
            .headers
            .get("authorization")
            .unwrap()
            .as_bytes()
            .to_vec();
        let mut out = auth.clone();
        out.extend(b"\n");
        out.extend(
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &auth).into_bytes(),
        );
        out.extend(b"\n");
        out.extend(body);
        let mut response = respond(200, out);
        response
            .headers_mut()
            .insert("x-echo", http::HeaderValue::from_bytes(&auth).unwrap());
        response
    })
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let request = Request::post("/echo?token=".to_owned() + &s.reference.to_string())
        .header("host", ALLOWED)
        .header("authorization", format!("Bearer {}", s.reference))
        .body(full(format!("{{\"key\":\"{}\"}}", s.reference)))
        .unwrap();
    let seen = https_via(&route, &s.authority, ALLOWED, echo.port, request).await;
    assert_eq!(seen.status, 200);
    assert_no_leak("the echoed response", &seen.all());
    let reference = s.reference.to_string();
    assert!(
        seen.text().starts_with(&format!("Bearer {reference}\n")),
        "{}",
        seen.text()
    );
    assert!(
        seen.text()
            .ends_with(&format!("{{\"key\":\"{reference}\"}}")),
        "{}",
        seen.text()
    );
    assert!(
        seen.headers
            .contains(&("x-echo".into(), format!("Bearer {reference}")))
    );
    assert_eq!(
        s.report().await.uses,
        vec![(s.reference, UseOutcome::Injected)]
    );
}

#[tokio::test]
async fn references_in_the_query_and_a_sized_body_are_swapped() {
    let mut s = setup();
    let got = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&got);
    let server = TestServer::https(Some(s.ca.server_config(&[ALLOWED])), move |req| {
        let log = Arc::clone(&log);
        async move {
            let (parts, body) = body_of(req).await;
            let length = parts
                .headers
                .get("content-length")
                .map(|v| v.to_str().unwrap().to_owned());
            log.lock()
                .unwrap()
                .push((parts.uri.to_string(), body, length));
            respond(204, "")
        }
    })
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let body = format!("client_secret={}&grant=x", s.reference);
    let request = Request::post(format!("/token?key={}&n=1", s.reference))
        .header("host", ALLOWED)
        .header("content-length", body.len())
        .body(full(body))
        .unwrap();
    let seen = https_via(&route, &s.authority, ALLOWED, server.port, request).await;
    assert_eq!(seen.status, 204);
    let (uri, body, length) = got.lock().unwrap()[0].clone();
    assert_eq!(uri, format!("/token?key={VALUE}&n=1"));
    let expected = format!("client_secret={VALUE}&grant=x");
    assert_eq!(String::from_utf8(body).unwrap(), expected);
    assert_eq!(length, Some(expected.len().to_string()));
    assert_eq!(
        s.report().await.uses,
        vec![(s.reference, UseOutcome::Injected)]
    );
}

#[tokio::test]
async fn a_lab_driver_and_a_project_not_allowed_get_nothing() {
    let mut s = setup_with(
        &[ALLOWED],
        Projects::Only(BTreeSet::from([ProjectId::from_u128(1)])),
        None,
    );
    let log = Log::default();
    let server = guarded(&s.ca, ALLOWED, Arc::clone(&log)).await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let seen = https_via(
        &route,
        &s.authority,
        ALLOWED,
        server.port,
        bearer(s.reference),
    )
    .await;
    assert_eq!(seen.status, 401);
    assert_eq!(
        s.report().await.uses,
        vec![(s.reference, UseOutcome::Refused(Refusal::ProjectNotAllowed))]
    );

    let mut s = setup();
    let server = guarded(&s.ca, ALLOWED, Arc::clone(&log)).await;
    let lab = Purpose::Lab {
        trial: TrialId::from_u128(3),
        arm: Arm::Changed,
        run: 1,
    };
    let route = s.proxy.open_route(requester(lab), &[]).await.unwrap();
    let seen = https_via(
        &route,
        &s.authority,
        ALLOWED,
        server.port,
        bearer(s.reference),
    )
    .await;
    assert_eq!(seen.status, 401);
    assert_eq!(
        s.report().await.uses,
        vec![(s.reference, UseOutcome::Refused(Refusal::LabDriver))]
    );
    assert!(log.lock().unwrap().iter().all(|a| !a.contains(VALUE)));
}

#[tokio::test]
async fn a_refused_host_gets_no_tunnel_and_is_reported() {
    let mut s = setup();
    let log = Log::default();
    let server = guarded(&s.ca, ALLOWED, Arc::clone(&log)).await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    s.proxy.refuse_host("Allowed.X100.test");
    let tcp = tokio::net::TcpStream::connect(route.addr()).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tcp))
            .await
            .unwrap();
    tokio::spawn(conn);
    let connect = Request::connect(format!("{ALLOWED}:{}", server.port))
        .header("proxy-authorization", proxy_auth(&route))
        .body(empty())
        .unwrap();
    assert_eq!(sender.send_request(connect).await.unwrap().status(), 403);
    let report = s.report().await;
    assert_eq!(
        (report.method.as_str(), report.outcome),
        ("CONNECT", EgressOutcome::HostRefused)
    );
    assert!(log.lock().unwrap().is_empty());

    s.proxy.allow_host(ALLOWED);
    let seen = https_via(
        &route,
        &s.authority,
        ALLOWED,
        server.port,
        bearer(s.reference),
    )
    .await;
    assert_eq!(seen.status, 200);
}

/// The host's own loopback holds the daemon and whatever else the builder runs locally, so a
/// sandbox cannot reach it through its proxy, by address or by name.
#[tokio::test]
async fn the_hosts_loopback_is_refused() {
    let mut s = setup();
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let tcp = tokio::net::TcpStream::connect(route.addr()).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tcp))
            .await
            .unwrap();
    tokio::spawn(conn);
    for target in ["127.0.0.1:22", "localhost:5432", "[::1]:80", "0.0.0.0:80"] {
        let connect = Request::connect(target)
            .header("proxy-authorization", proxy_auth(&route))
            .body(empty())
            .unwrap();
        assert_eq!(
            sender.send_request(connect).await.unwrap().status(),
            403,
            "{target}"
        );
        assert_eq!(s.report().await.outcome, EgressOutcome::HostRefused);
    }
}

#[tokio::test]
async fn a_request_without_the_routes_token_is_challenged() {
    let s = setup();
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    for auth in [None, Some("Basic eDEwMDp3cm9uZw==".to_owned())] {
        let tcp = tokio::net::TcpStream::connect(route.addr()).await.unwrap();
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tcp))
                .await
                .unwrap();
        tokio::spawn(conn);
        let mut connect = Request::connect(format!("{ALLOWED}:443"));
        if let Some(auth) = &auth {
            connect = connect.header("proxy-authorization", auth);
        }
        let answer = sender
            .send_request(connect.body(empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(answer.status(), 407);
        assert_eq!(
            answer.headers()["proxy-authenticate"],
            "Basic realm=\"x100\""
        );
    }
}

#[tokio::test]
async fn plain_http_goes_through_as_absolute_form_requests() {
    let mut s = setup();
    let log = Log::default();
    let logged = Arc::clone(&log);
    let server = TestServer::https(None, move |req| {
        let log = Arc::clone(&logged);
        async move {
            let auth = req
                .headers()
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default();
            log.lock().unwrap().push(format!("{} {}", req.uri(), auth));
            respond(200, "plain")
        }
    })
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let tcp = tokio::net::TcpStream::connect(route.addr()).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tcp))
            .await
            .unwrap();
    tokio::spawn(conn);
    let request = Request::get(format!("http://{ALLOWED}:{}/x?y=1", server.port))
        .header("host", ALLOWED)
        .header("proxy-authorization", proxy_auth(&route))
        .header("authorization", format!("Bearer {}", s.reference))
        .body(empty())
        .unwrap();
    let seen = read(sender.send_request(request).await.unwrap()).await;
    assert_eq!((seen.status, seen.text().as_str()), (200, "plain"));
    assert_eq!(*log.lock().unwrap(), vec![format!("/x?y=1 Bearer {VALUE}")]);
    assert_eq!(
        s.report().await.uses,
        vec![(s.reference, UseOutcome::Injected)]
    );
}

#[tokio::test]
async fn an_upstream_whose_certificate_does_not_check_out_is_not_reached() {
    // The proxy trusts the system's authorities only, and the local server's is not among them.
    let mut s = setup_with(&[ALLOWED], Projects::All, Some(UpstreamTrust::System));
    let log = Log::default();
    let server = guarded(&s.ca, ALLOWED, Arc::clone(&log)).await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let seen = https_via(
        &route,
        &s.authority,
        ALLOWED,
        server.port,
        bearer(s.reference),
    )
    .await;
    assert_eq!(seen.status, 502);
    let report = s.report().await;
    assert!(
        matches!(&report.outcome, EgressOutcome::Failed(e) if e.contains("certificate") || e.contains("UnknownIssuer")),
        "{report:?}"
    );
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn dropping_a_route_closes_its_listener() {
    let s = setup();
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let addr = route.addr();
    drop(route);
    tokio::task::yield_now().await;
    let mut closed = false;
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_err() {
            closed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(closed);
}

#[tokio::test]
async fn a_pass_through_host_is_relayed_untouched_with_tls_end_to_end() {
    let mut s = setup();
    let log = Log::default();
    let server = guarded(&s.ca, ALLOWED, Arc::clone(&log)).await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &["Allowed.x100.test"])
        .await
        .unwrap();
    // The client trusts the host's own authority, not the proxy's: the proxy is not in the
    // middle.
    let seen = https_via(
        &route,
        &s.ca.cert,
        ALLOWED,
        server.port,
        bearer(s.reference),
    )
    .await;
    assert_eq!(seen.status, 401);
    assert_eq!(
        *log.lock().unwrap(),
        vec![format!("Bearer {}", s.reference)]
    );
    let report = s.report().await;
    assert_eq!(
        (report.method.as_str(), &report.outcome, report.uses.len()),
        ("CONNECT", &EgressOutcome::Tunnelled, 0)
    );
}
