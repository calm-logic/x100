//! The endpoint Claude Code's `http` hooks post to. Each driver listens on a port of its own on
//! localhost and accepts only calls that carry its secret, so another process cannot pose as its
//! harness.

use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::http;
use crate::protocol::HookCall;
use crate::wire::Incoming;

/// The header that carries the driver's hook secret.
pub(super) const SECRET_HEADER: &str = "X-X100-Hook";

/// Hooks whose answer Claude Code waits on. Every other hook is answered at once with nothing.
const ANSWERED: &[&str] = &["PermissionRequest"];

/// Listens on a free port on localhost; returns the port and the task serving it.
pub(super) async fn serve(
    secret: String,
    incoming: mpsc::UnboundedSender<Incoming>,
) -> std::io::Result<(u16, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(answer(stream, secret.clone(), incoming.clone()));
        }
    });
    Ok((port, task))
}

async fn answer(mut stream: TcpStream, secret: String, incoming: mpsc::UnboundedSender<Incoming>) {
    let Ok(request) = http::read_request(&mut stream).await else {
        return;
    };
    let event = request
        .target
        .strip_prefix("/hook/")
        .map(str::to_owned)
        .filter(|_| request.method == "POST");
    let (Some(event), true) = (
        event,
        request.header(SECRET_HEADER) == Some(secret.as_str()),
    ) else {
        let _ = http::respond(&mut stream, 403, "application/json", b"").await;
        return;
    };
    let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
    let (reply, answered) = if ANSWERED.contains(&event.as_str()) {
        let (tx, rx) = oneshot::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    // Sent before the answer, so the harness cannot call its next hook before this one is heard.
    let _ = incoming.send(Incoming::Hook(HookCall { event, body, reply }));
    let answer = match answered {
        Some(rx) => rx
            .await
            .map(|v| v.to_string().into_bytes())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let _ = http::respond(&mut stream, 200, "application/json", &answer).await;
}
