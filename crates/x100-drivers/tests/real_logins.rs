//! Each terminal harness on the builder's own local login. These use the builder's subscription,
//! so they are ignored by default and run only when asked for:
//!
//! ```sh
//! X100_REAL_LOGINS=1 cargo test -p x100-drivers --test real_logins -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Each makes two turns on the cheapest model at low effort, in a temporary directory: a one-file
//! edit, then a question only the first turn's context answers. The login is the builder's
//! `~/.claude` or `~/.codex`, which the harness reads itself; the test only names the directory.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::terminal::Watched;
use common::*;
use x100_core::effort::Effort;
use x100_core::time::{FakeClock, Timestamp};
use x100_drivers::claude::{ClaudeTerminal, ClaudeTerminalConfig};
use x100_drivers::codex::{CodexTerminal, CodexTerminalConfig};
use x100_drivers::{DriverOutput, Harness, TurnEnd, group_is_empty};

const EDIT: &str = "Create a file named hello.txt in the current directory containing exactly one \
                    line: hello from x100. Then reply with the single word DONE.";
const RECALL: &str =
    "What is the name of the file you just created? Reply with the file name only.";

/// The login's directory under the builder's home, if the run was asked for.
fn opted_in(dir: &str) -> Option<PathBuf> {
    if std::env::var("X100_REAL_LOGINS").as_deref() != Ok("1") {
        eprintln!("SKIPPED: set X100_REAL_LOGINS=1 to run on the builder's own login");
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let login = home.join(dir);
    assert!(login.is_dir(), "no login directory at {}", login.display());
    Some(login)
}

/// What the login said about its usage windows, read from the driver's own directory under
/// `state` once the turns are done.
type Usage = fn(&std::path::Path) -> std::pin::Pin<Box<dyn std::future::Future<Output = String>>>;

async fn edit_and_recall(
    harness: &dyn Harness,
    model: &str,
    login: PathBuf,
    state: &std::path::Path,
    usage: Usage,
) {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut spec = spec(1, &work, model, Effort::Low);
    spec.state = state.join(x100_core::id::DriverId::from_u128(1).to_string());
    spec.login = Some(login);
    let handle = match harness.start(spec).await {
        Ok(handle) => handle,
        Err(e) => panic!("the driver did not start: {e}"),
    };
    eprintln!("started: {:?}", handle.info());
    let mut w = Watched::new(handle);

    w.handle.turn(EDIT).await.unwrap();
    let first = w.until_end().await;
    eprintln!("first turn: {first:#?}\nscreen:\n{}", w.replay());
    assert_eq!(
        first.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed)),
        "{first:#?}"
    );
    let written = std::fs::read_to_string(work.join("hello.txt")).unwrap();
    assert_eq!(written.trim(), "hello from x100");

    w.handle.turn(RECALL).await.unwrap();
    let second = w.until_end().await;
    eprintln!("second turn: {second:#?}\nscreen:\n{}", w.replay());
    let said: String = second
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        said.contains("hello.txt"),
        "the second turn lost the first: {second:#?}"
    );
    w.assert_replay_matches().await;
    let driver = state.join(x100_core::id::DriverId::from_u128(1).to_string());
    eprintln!("usage as the login reports it: {}", usage(&driver).await);
    let warnings: Vec<_> = first
        .iter()
        .chain(&second)
        .filter(|o| matches!(o, DriverOutput::Warning(_)))
        .collect();
    eprintln!("warnings: {warnings:?}");

    let pgid = w.handle.info().pid;
    w.handle.stop().await;
    while w.next().await.is_some() {}
    eventually("the process group to empty", || group_is_empty(pgid)).await;
}

#[tokio::test]
#[ignore = "runs on the builder's own Claude login; see the module docs"]
async fn claude_in_a_terminal_on_the_builders_login() {
    let Some(login) = opted_in(".claude") else {
        return;
    };
    let program = installed("claude").expect("claude must be installed for this test");
    let state = tempfile::tempdir().unwrap();
    let mut config = ClaudeTerminalConfig::new();
    config.program = program;
    let harness = ClaudeTerminal::new(config, Arc::new(FakeClock::new(Timestamp::from_millis(1))));
    edit_and_recall(
        &harness,
        "claude-haiku-5-5",
        login,
        state.path(),
        |driver| {
            let path = driver.join("x100/status.json");
            Box::pin(async move {
                let status: serde_json::Value = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default();
                format!("rate_limits {}", status["rate_limits"])
            })
        },
    )
    .await;
}

#[tokio::test]
#[ignore = "runs on the builder's own Codex login; see the module docs"]
async fn codex_in_a_terminal_on_the_builders_login() {
    let Some(login) = opted_in(".codex") else {
        return;
    };
    let program = installed("codex").expect("codex must be installed for this test");
    let state = tempfile::tempdir().unwrap();
    let mut config = CodexTerminalConfig::new();
    config.program = program;
    let harness = CodexTerminal::new(config, Arc::new(FakeClock::new(Timestamp::from_millis(1))));
    edit_and_recall(&harness, "gpt-6-luna", login, state.path(), |driver| {
        let socket = driver.join("socket/app.sock");
        Box::pin(async move { read_codex_limits(&socket).await })
    })
    .await;
}

/// Asks the driver's app-server, as a second client, what the login's usage windows stand at.
async fn read_codex_limits(socket: &std::path::Path) -> String {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let real = std::fs::canonicalize(socket).unwrap();
    let stream = tokio::net::UnixStream::connect(real).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .unwrap();
    let send = |v: serde_json::Value| Message::text(v.to_string());
    ws.send(send(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "clientInfo": { "name": "x100-test", "title": null, "version": "0" }, "capabilities": null } })))
        .await
        .unwrap();
    ws.send(send(
        serde_json::json!({ "jsonrpc": "2.0", "method": "initialized" }),
    ))
    .await
    .unwrap();
    ws.send(send(
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "account/rateLimits/read",
        "params": { "excludeResetCreditDetails": true } }),
    ))
    .await
    .unwrap();
    while let Some(Ok(frame)) = ws.next().await {
        if let Message::Text(text) = frame {
            let message: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
            if message["id"] == 2 {
                return message.to_string();
            }
        }
    }
    "no answer".into()
}
