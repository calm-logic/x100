//! What a lab driver can reach, through the daemon as a child process: it runs in a sandbox of its
//! own whose only way out is the vault's proxy, on a route that is a lab driver's. A credential
//! the same request would carry for a work driver is refused it, a request that would write is
//! refused before anything goes out, and nothing gets past the proxy. Each is recorded.
//!
//! The proxy sends a made-up host to a test server on loopback, which records every request. The
//! credential may go to that host from any project, so only the driver being a lab driver keeps
//! it out. Needs a sandbox; on a machine that cannot make one it is passed over.

mod common;
#[path = "lab/rig.rs"]
mod rig;

use std::sync::{Arc, Mutex};

use common::available::skip_without_sandbox;
use rig::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use x100_core::action::{ActionEvent, Request};
use x100_core::credential::{CredentialScope, Projects, Refusal, UseOutcome, Via};
use x100_core::driver::Purpose;
use x100_core::event::ProjectEvent;
use x100_core::transcript::Item;
use x100_proto::{AddCredential, ReadTranscript, Secret};

/// Invented for this test; it opens nothing anywhere.
const VALUE: &str = "x100-lab-dummy-5b4a3928c1d0e9f8+7766/aa=";
const HOST: &str = "api.x100.test";

/// What the server was sent: each request's method, path and `Authorization`.
type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

async fn server() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let mut first = head.lines().next().unwrap_or("").split(' ');
                let (method, path) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
                let auth = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("authorization")
                            .then(|| v.trim().to_owned())
                    })
                    .unwrap_or_default();
                log.lock()
                    .unwrap()
                    .push((method.to_owned(), path.to_owned(), auth.clone()));
                let status = if auth == format!("Bearer {VALUE}") {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                };
                let reply =
                    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, seen)
}

/// The lab drivers' turn: a read carrying the reference, a write, and a request that tries to go
/// around the proxy.
fn lab_turn(reference: &str, port: u16) -> String {
    format!(
        r#"
[[turn]]
when = {{ start_pack_contains = "LAB RUN", input_contains = "Write the greeting" }}
repeat = true
steps = [
  {{ run = "curl -s -o /dev/null -w 'READ %{{http_code}}' -H 'Authorization: Bearer {reference}' http://{HOST}:{port}/read; echo; curl -s -o /dev/null -w 'WRITE %{{http_code}}' -X POST -d 'x=1' -H 'Authorization: Bearer {reference}' http://{HOST}:{port}/write; echo; curl -s -o /dev/null --max-time 3 -w 'AROUND %{{http_code}}' --noproxy '*' http://127.0.0.1:{port}/around; echo" }},
  {{ usage = {{ input = 10, output = 5, cost_micros = 2000 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "tried" }} }} }},
]
"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lab_driver_is_refused_credentials_and_outbound_writes_and_cannot_go_around_the_proxy() {
    if skip_without_sandbox() {
        return;
    }
    let (port, seen) = server().await;
    let reflection = [
        r#"  { call = { propose_change = { trial = { change = { new_skill = { skill = { scope = "shared", name = "greeting" }, body = ""#,
        &skill_body("greeting", "Greet."),
        r#"" } }, eval = { items = [ { past_task = { task = "{{input:Accepted}}", checkpoint = "" } } ], runs_per_arm = 3 } } } }, save = "t" },"#,
    ]
    .concat();
    let resolve = format!("{HOST}=127.0.0.1");
    let r = Rig::start(
        "isolated",
        &script(&reflection, ""),
        &[(x100d::PROXY_RESOLVE_ENV, resolve)],
    )
    .await;
    let reference = r
        .client
        .request(AddCredential {
            name: "Test API".into(),
            value: Secret::new(VALUE),
            scope: CredentialScope {
                hosts: [HOST.to_owned()].into(),
                commands: Default::default(),
                projects: Projects::All,
            },
        })
        .await
        .expect("the credential is added")
        .credential;
    // Lab drivers read the script when they start, after the credential exists.
    std::fs::write(
        r.env.scripts.join("fake.toml"),
        script(&reflection, &lab_turn(&reference.to_string(), port)),
    )
    .unwrap();
    r.set_trials(true).await;
    r.reflect().await;

    let lab = r.lab_drivers();
    assert_eq!(lab.len(), 6, "three runs of each arm");
    let got = seen.lock().unwrap().clone();
    let reads: Vec<_> = got.iter().filter(|(m, ..)| m == "GET").collect();
    assert_eq!(reads.len(), 6, "each lab driver's read went out: {got:?}");
    for (_, path, auth) in &reads {
        assert_eq!(path, "/read");
        assert_eq!(
            auth,
            &format!("Bearer {reference}"),
            "the reference, never the value"
        );
    }
    assert!(
        got.iter().all(|(m, ..)| m == "GET"),
        "no write and nothing around the proxy reached the server: {got:?}"
    );
    assert!(!got.iter().any(|(_, _, a)| a.contains(VALUE)));

    // Each refusal is in the log, as the lab driver's.
    let events = r.events().await;
    let lab_ids: Vec<_> = lab.iter().map(|d| d.driver.id).collect();
    let refused_uses = events
        .iter()
        .filter(|e| {
            matches!(&e.event, ProjectEvent::CredentialUsed(u)
                if lab_ids.contains(&u.driver)
                && u.outcome == UseOutcome::Refused(Refusal::LabDriver)
                && matches!(&u.via, Via::Proxy { host } if host == HOST))
        })
        .count();
    assert_eq!(
        refused_uses, 6,
        "a credential is refused to every lab driver"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(&e.event, ProjectEvent::CredentialUsed(u)
            if lab_ids.contains(&u.driver) && u.outcome == UseOutcome::Injected)),
        "no lab driver was given a credential"
    );
    let writes: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::ActionProposed(a) => match &a.request {
                Request::Egress { driver, method, .. }
                    if lab_ids.contains(driver) && method == "POST" =>
                {
                    Some(a.id)
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 6, "each write is an action");
    for id in writes {
        let ended = events.iter().find_map(|e| match &e.event {
            ProjectEvent::Action {
                action,
                event: ActionEvent::Failed { error },
            } if *action == id => Some(error.clone()),
            _ => None,
        });
        assert!(
            ended.as_deref().is_some_and(|w| w.contains("never writes")),
            "{ended:?}"
        );
    }

    // What the lab drivers saw: their read answered, their write refused by the proxy, and no
    // way around it.
    let items: Vec<Item> = r
        .client
        .request(ReadTranscript {
            project: r.project,
            agent: r.ceo,
            after: None,
            limit: 1000,
        })
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| lab_ids.contains(&e.entry.driver))
        .map(|e| e.entry.item)
        .collect();
    let outputs: Vec<&str> = items
        .iter()
        .filter_map(|i| match i {
            Item::ToolResult { output, .. } if output.contains("READ") => Some(output.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), 6, "{items:?}");
    for out in outputs {
        assert!(out.contains("READ 401"), "{out}");
        assert!(out.contains("WRITE 403"), "{out}");
        assert!(out.contains("AROUND 000"), "{out}");
    }
    assert!(
        lab.iter()
            .all(|d| matches!(d.driver.purpose, Purpose::Lab { .. }))
    );
}
