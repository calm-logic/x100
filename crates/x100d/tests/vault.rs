//! M11's check through the daemon as a child process: an agent makes an authenticated request
//! using only a reference, and succeeds. Searching the event log, the agent's transcript and the
//! sandbox's files for the value finds nothing, including base64 and URL-encoded forms. A request
//! to a host the credential is not allowed on goes out without it.
//!
//! The daemon's proxy sends two made-up hosts to a test server on loopback and refuses every
//! other host. The server lets in only a request that carries the value, and echoes what it was
//! sent. The fake harness's driver runs `curl` in its sandbox with the reference. No rule knows
//! either host and no decision model is connected, so the gate lets both reads through at the
//! lowest risk a read can have.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use common::{DaemonProc, Env, WAIT, create_project};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use x100_client::Client;
use x100_core::credential::{CredentialScope, Projects, Refusal, UseOutcome, Via};
use x100_core::event::ProjectEvent;
use x100_core::id::{CredentialRef, ProjectId};
use x100_core::transcript::Item;
use x100_proto::{
    AddCredential, LogId, LogPage, Page, ReadLog, ReadTranscript, RemoveCredential, Secret,
};

/// Invented for this test; it opens nothing anywhere. Its `+`, `/` and `=` differ in every
/// encoding searched for.
const VALUE: &str = "x100-dummy-9f8e7d6c5b4a3928+1706/f5e4d3c2b1a0=";
const ALLOWED: &str = "api.x100.test";
const OTHER: &str = "other.x100.test";

const SCRIPT: &str = r#"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "CALL" }
repeat = true
steps = [
  { run = "curl -s -w ' %{http_code}' -H 'Authorization: Bearer {{input:Ref}}' http://api.x100.test:{{input:Port}}/ > allowed.txt; curl -s -w ' %{http_code}' -H 'Authorization: Bearer {{input:Ref}}' http://other.x100.test:{{input:Port}}/ > other.txt; cat allowed.txt; echo; cat other.txt" },
  { call = { message = { to = "builder", body = "called", attachments = [] } } },
]

[[turn]]
repeat = true
steps = [{ text = "nothing to do" }]
"#;

/// What the server was sent: the `Host` and `Authorization` of each request.
type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Answers 200 only to the value, and echoes the authorization it got either way.
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
                let header = |name: &str| {
                    head.lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
                        })
                        .unwrap_or_default()
                };
                let (host, auth) = (header("host"), header("authorization"));
                log.lock().unwrap().push((host, auth.clone()));
                let (status, body) = if auth == format!("Bearer {VALUE}") {
                    ("200 OK", format!("welcome, holder of {VALUE}"))
                } else {
                    ("401 Unauthorized", format!("refused {auth}"))
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, seen)
}

/// Every form of the value the search looks for, worked out here apart from the redactor's own:
/// raw, percent-encoded with either case of hex, hex in either case, and base64 standard and
/// URL-safe at each of the three alignments, keeping only the characters the value alone decides.
fn forms(value: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![value.to_vec()];
    let hex: String = value.iter().map(|b| format!("{b:02x}")).collect();
    out.push(hex.clone().into_bytes());
    out.push(hex.to_uppercase().into_bytes());
    for upper in [true, false] {
        let encoded: String = value
            .iter()
            .map(|&b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ if upper => format!("%{b:02X}"),
                _ => format!("%{b:02x}"),
            })
            .collect();
        out.push(encoded.into_bytes());
    }
    for engine in [&STANDARD, &URL_SAFE] {
        for k in 0..3usize {
            let mut padded = vec![0u8; k];
            padded.extend_from_slice(value);
            padded.extend_from_slice(&[0, 0]);
            let text = engine.encode(&padded);
            let first = (8 * k).div_ceil(6);
            let last = (8 * (k + value.len()) - 6) / 6;
            out.push(text.as_bytes()[first..=last].to_vec());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Each file under `dir` that holds any form of the value, with the form found.
fn holding(dir: &Path, forms: &[Vec<u8>]) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && let Ok(bytes) = std::fs::read(&path)
            {
                for f in forms {
                    if bytes.windows(f.len()).any(|w| w == f.as_slice()) {
                        found.push((path.clone(), String::from_utf8_lossy(f).into_owned()));
                    }
                }
            }
        }
    }
    found
}

fn spawn(env: &Env) -> DaemonProc {
    let resolve = format!("{ALLOWED}=127.0.0.1,{OTHER}=127.0.0.1");
    DaemonProc::spawn_adding(
        env.daemon_args(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness"))),
        &[(x100d::PROXY_RESOLVE_ENV, resolve)],
    )
}

async fn call(client: &Client, project: ProjectId, reference: CredentialRef, port: u16) {
    let before = client.project(project).unwrap().messages().count();
    let body = format!("CALL\nRef: {reference}\nPort: {port}");
    client
        .send_chat(project, &body, vec![], None)
        .await
        .unwrap();
    client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            (p.messages()
                .skip(before)
                .any(|m| m.message.body == "called"))
            .then_some(())
        })
        .await
        .expect("the driver called");
}

async fn uses(client: &Client, project: ProjectId) -> Vec<(String, UseOutcome)> {
    let page = client
        .request(ReadLog {
            log: LogId::Project(project),
            page: Page::OldestFirst {
                after: None,
                limit: 1000,
            },
        })
        .await
        .unwrap();
    let LogPage::Project { events } = page else {
        panic!("a project's log")
    };
    events
        .into_iter()
        .filter_map(|e| match e.event {
            ProjectEvent::CredentialUsed(u) => match u.via {
                Via::Proxy { host } => Some((host, u.outcome)),
                Via::Wrapper { .. } => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reference_works_only_where_allowed_and_the_value_is_found_nowhere() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let env = Env::new();
    std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
    let (port, seen) = server().await;
    let mut daemon = spawn(&env);
    let client = env.client().await;

    let added = client
        .request(AddCredential {
            name: "Test API".into(),
            value: Secret::new(VALUE),
            scope: CredentialScope {
                hosts: [ALLOWED.to_owned()].into(),
                commands: Default::default(),
                projects: Projects::All,
            },
        })
        .await
        .expect("the credential is added");
    let reference = added.credential;
    assert_eq!(
        serde_json::to_value(&added).unwrap(),
        serde_json::json!({ "credential": reference }),
        "the answer is the reference and nothing else"
    );
    let listed = client
        .wait_for(WAIT, |s| s.daemon.credentials().next().cloned())
        .await
        .unwrap();
    assert_eq!(listed.credential.name, "Test API");
    // A value too short to tell apart in output is refused.
    let short = client
        .request(AddCredential {
            name: "short".into(),
            value: Secret::new("abc"),
            scope: listed.credential.scope.clone(),
        })
        .await;
    assert!(short.is_err());

    let project = create_project(&client, &env, "calls").await;
    call(&client, project, reference, port).await;

    // The allowed host got the value and let the request in; the other got the reference as it
    // was written.
    let got = seen.lock().unwrap().clone();
    let to = |host: &str| -> Vec<String> {
        got.iter()
            .filter(|(h, _)| h.starts_with(host))
            .map(|(_, a)| a.clone())
            .collect()
    };
    assert_eq!(to(ALLOWED), [format!("Bearer {VALUE}")], "{got:?}");
    assert_eq!(to(OTHER), [format!("Bearer {reference}")], "{got:?}");
    let ceo = client.project(project).unwrap().project().unwrap().ceo;
    let work = x100_store::Git::new(PathBuf::from(env.project_dir("calls"))).worktree_path(ceo);
    let allowed = std::fs::read_to_string(work.join("allowed.txt")).unwrap();
    assert_eq!(allowed, format!("welcome, holder of {reference} 200"));
    let other = std::fs::read_to_string(work.join("other.txt")).unwrap();
    assert!(other.ends_with(" 401"), "{other}");

    // Each use is in the project's log: injected where allowed, refused elsewhere.
    let used = uses(&client, project).await;
    assert!(
        used.contains(&(ALLOWED.to_owned(), UseOutcome::Injected)),
        "{used:?}"
    );
    assert!(
        used.contains(&(
            OTHER.to_owned(),
            UseOutcome::Refused(Refusal::HostNotAllowed)
        )),
        "{used:?}"
    );

    // The value the server echoed reached the transcript as the reference.
    let items: Vec<Item> = client
        .request(ReadTranscript {
            project,
            agent: ceo,
            after: None,
            limit: 1000,
        })
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.entry.item)
        .collect();
    let echoed = items.iter().any(|i| {
        matches!(i, Item::ToolResult { output, .. }
            if output.contains(&format!("welcome, holder of {reference}")))
    });
    assert!(echoed, "{items:?}");

    // No form of the value is anywhere: the logs and transcripts, the sandbox's home, its /tmp
    // and its worktree, and the whole of the daemon's data directory, the vault's sealed files
    // among them.
    let forms = forms(VALUE.as_bytes());
    // Raw, hex twice, percent-encoded twice, and base64 at three alignments, where this value
    // gives the same text in both alphabets.
    assert!(forms.len() >= 8, "{}", forms.len());
    // The search finds a form it is looking for, here base64 off its usual alignment.
    let control = env.dir.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    let mut shifted = b"z".to_vec();
    shifted.extend_from_slice(VALUE.as_bytes());
    std::fs::write(control.join("planted"), STANDARD.encode(&shifted)).unwrap();
    assert_eq!(holding(&control, &forms).len(), 1);
    let sandboxes = env.data.join("sb");
    assert!(
        std::fs::read_dir(&sandboxes).unwrap().count() > 0,
        "the sandbox is there to search"
    );
    for dir in [env.data.clone(), PathBuf::from(env.project_dir("calls"))] {
        let found = holding(&dir, &forms);
        assert!(found.is_empty(), "{found:?}");
    }

    // The vault's files are the builder's alone.
    for file in ["vault.sealed", "vault.key"] {
        let mode = std::fs::metadata(env.data.join("vault").join(file))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{file} is {mode:o}");
    }

    // After a restart the credential is still listed and still works.
    daemon.terminate();
    for dir in [env.data.clone(), PathBuf::from(env.project_dir("calls"))] {
        let found = holding(&dir, &forms);
        assert!(found.is_empty(), "after the daemon stopped: {found:?}");
    }
    seen.lock().unwrap().clear();
    let mut daemon = spawn(&env);
    let client = env.client().await;
    let listed = client
        .wait_for(WAIT, |s| s.daemon.credentials().next().cloned())
        .await
        .unwrap();
    assert_eq!(listed.credential.reference, reference);
    client
        .wait_for(WAIT, |s| s.projects.get(&project).map(|_| ()))
        .await
        .unwrap();
    call(&client, project, reference, port).await;
    let got = seen.lock().unwrap().clone();
    assert!(
        got.contains(&(format!("{ALLOWED}:{port}"), format!("Bearer {VALUE}"))),
        "{got:?}"
    );

    // Removed, it is gone from the list and the reference reaches the server unreplaced.
    client
        .request(RemoveCredential {
            credential: reference,
        })
        .await
        .unwrap();
    client
        .wait_for(WAIT, |s| {
            s.daemon.credentials().next().is_none().then_some(())
        })
        .await
        .unwrap();
    seen.lock().unwrap().clear();
    call(&client, project, reference, port).await;
    let got = seen.lock().unwrap().clone();
    assert!(
        got.iter().all(|(_, a)| a == &format!("Bearer {reference}")),
        "{got:?}"
    );
    daemon.terminate();
    let found = holding(&env.data, &forms);
    assert!(found.is_empty(), "{found:?}");
}
