//! The daemon serves its tools over MCP on a loopback port of its own while it runs, refuses a
//! caller it gave no token, and closes the port when it stops.

mod common;

use common::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use x100_core::id::DriverId;

/// One JSON-RPC request over a fresh HTTP/1.1 connection, and the body of the answer.
async fn post(url: &str, path: &str, token: &str, body: Value) -> std::io::Result<String> {
    let authority = url.strip_prefix("http://").expect("an http address");
    let mut stream = TcpStream::connect(authority).await?;
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nAuthorization: Bearer {token}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await?;
    Ok(answer)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_serves_its_tools_on_loopback_and_refuses_strangers() {
    let env = Env::new();
    let mut config = x100d::Config::new(&env.data);
    config.scripts_dir = Some(env.scripts.clone());
    config.fake_program = Some(env!("CARGO_BIN_EXE_x100d-fake-harness").into());
    let daemon = x100d::Daemon::start(config)
        .await
        .expect("the daemon starts");
    let url = daemon.tools_url().to_owned();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let stop = daemon.stop_handle();
    let task = tokio::spawn(daemon.run());

    let stranger = DriverId::from_u128(42);
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
    let answer = post(
        &url,
        &format!("/drivers/{stranger}/mcp"),
        "made-up",
        list.clone(),
    )
    .await
    .expect("the tools server answers");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.contains("was not given to"), "{answer}");
    let nowhere = post(&url, "/elsewhere", "made-up", list.clone())
        .await
        .unwrap();
    assert!(nowhere.starts_with("HTTP/1.1 404"), "{nowhere}");

    stop.now();
    task.await.unwrap();
    assert!(
        post(&url, &format!("/drivers/{stranger}/mcp"), "made-up", list)
            .await
            .is_err(),
        "the port is closed once the daemon has stopped"
    );
}
