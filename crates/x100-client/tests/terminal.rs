//! `follow_terminal` against a scripted daemon on a socket: the client's side of
//! `terminal.subscribe` and `terminal.chunk`, which the real daemon does not serve yet.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use x100_client::{Change, Client, ClientError, Options};
use x100_core::id::{DriverId, ProjectId};
use x100_core::terminal::{Chunk, ChunkKind, TermPos};
use x100_core::time::Timestamp;
use x100_proto::{
    Bytes, Call, ErrorCode, Id, Message, Notice, Response, RpcError, Snapshot, SubscriptionId,
    TerminalNotice,
};

const PROJECT: ProjectId = ProjectId::from_u128(1);
/// Streams two chunks, repeats one, and drops the connection; then streams the third.
const FOLLOWED: DriverId = DriverId::from_u128(2);
const UNSERVED: DriverId = DriverId::from_u128(3);
const HEADLESS: DriverId = DriverId::from_u128(4);
const WAIT: Duration = Duration::from_secs(10);

/// Each `terminal.subscribe` the daemon was sent: which driver, and after which chunk.
type Asked = Arc<Mutex<Vec<(DriverId, Option<TermPos>)>>>;

fn chunk(position: u64) -> (TermPos, Chunk) {
    let what = if position == 1 {
        ChunkKind::Resize { cols: 80, rows: 24 }
    } else {
        ChunkKind::Output {
            bytes: Bytes(format!("line {position}\r\n").into_bytes()),
        }
    };
    (
        TermPos(position),
        Chunk {
            at: Timestamp::from_millis(1_000 + position),
            what,
        },
    )
}

/// The daemon's side: answers the handshake, and for each `terminal.subscribe` records the
/// position asked for and does what the driver stands for.
async fn serve(listener: UnixListener, asked: Asked) {
    let mut connection = 0;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        connection += 1;
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(Message::Request(request)) = Message::parse(&line) else {
                panic!("not a request: {line}");
            };
            let id = request.id.clone();
            let mut out = Vec::new();
            let mut hang_up = false;
            match request.call {
                Call::Hello(_) => {
                    out.push(ok(id, serde_json::json!({"daemon": "fake", "protocol": 1})))
                }
                Call::Subscribe(s) => {
                    let snapshot = s.resume.is_none().then(Snapshot::default);
                    out.push(ok(
                        id,
                        serde_json::json!({"subscription": 1, "snapshot": snapshot}),
                    ));
                }
                Call::SubscribeTerminal(s) if s.driver == FOLLOWED => {
                    asked.lock().unwrap().push((s.driver, s.after));
                    out.push(ok(id, serde_json::json!({"subscription": 7})));
                    if connection == 1 {
                        // The second chunk twice, as after a resubscribe that overlaps.
                        for p in [1, 2, 2] {
                            out.push(notice(7, chunk(p)));
                        }
                        hang_up = true;
                    } else {
                        // Starts over from the first chunk: the client must skip what it has.
                        for p in 1..=3 {
                            out.push(notice(7, chunk(p)));
                        }
                    }
                }
                Call::SubscribeTerminal(s) if s.driver == UNSERVED => {
                    asked.lock().unwrap().push((s.driver, s.after));
                    out.push(err(
                        id,
                        ErrorCode::Unavailable,
                        "terminal.subscribe is not available yet: it arrives with M3T",
                    ))
                }
                Call::SubscribeTerminal(s) => {
                    asked.lock().unwrap().push((s.driver, s.after));
                    out.push(err(id, ErrorCode::NotFound, "no terminal for that driver"))
                }
                Call::Unsubscribe(_) => out.push(ok(id, serde_json::json!({}))),
                other => panic!("unexpected {}", other.method()),
            }
            for line in out {
                write.write_all(line.as_bytes()).await.unwrap();
                write.write_all(b"\n").await.unwrap();
            }
            if hang_up {
                break;
            }
        }
    }
}

fn ok(id: Id, result: serde_json::Value) -> String {
    Message::Response(Response {
        id: Some(id),
        outcome: Ok(result),
    })
    .to_line()
}

fn err(id: Id, code: ErrorCode, message: &str) -> String {
    Message::Response(Response::err(Some(id), RpcError::new(code, message))).to_line()
}

fn notice(subscription: u64, (position, chunk): (TermPos, Chunk)) -> String {
    Message::Notice(Notice::TerminalChunk(TerminalNotice {
        subscription: SubscriptionId(subscription),
        position,
        chunk,
    }))
    .to_line()
}

async fn start() -> (tempfile::TempDir, Client, Asked) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("x100d.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(serve(listener, Arc::clone(&asked)));
    let client = Client::connect(Options::new(&socket, "test"))
        .await
        .unwrap();
    (dir, client, asked)
}

#[tokio::test]
async fn a_terminal_is_followed_across_a_reconnect_each_chunk_once() {
    let (_dir, client, asked) = start().await;
    let mut terminal = client
        .follow_terminal(PROJECT, FOLLOWED, None)
        .await
        .expect("followed");
    let mut got = Vec::new();
    for _ in 0..3 {
        let next = tokio::time::timeout(WAIT, terminal.next()).await;
        got.push(next.expect("a chunk in time").expect("the stream is open"));
    }
    assert_eq!(got, (1..=3).map(chunk).collect::<Vec<_>>());
    // The resubscribe after the hang-up continued from the last chunk handed on.
    assert_eq!(
        *asked.lock().unwrap(),
        vec![(FOLLOWED, None), (FOLLOWED, Some(TermPos(2)))]
    );
}

#[tokio::test]
async fn a_daemon_that_cannot_serve_terminals_yet_says_so() {
    let (_dir, client, asked) = start().await;
    let refused = client.follow_terminal(PROJECT, UNSERVED, None).await;
    assert!(
        matches!(&refused, Err(ClientError::Rpc(e)) if e.code == ErrorCode::Unavailable),
        "{:?}",
        refused.err()
    );
    let missing = client
        .follow_terminal(PROJECT, HEADLESS, Some(TermPos(4)))
        .await;
    assert!(
        matches!(&missing, Err(ClientError::Rpc(e)) if e.code == ErrorCode::NotFound),
        "{:?}",
        missing.err()
    );
    // Neither refusal is followed again: a reconnect resubscribes nothing.
    let mut changes = client.changes();
    client.reconnect();
    let back =
        async { while changes.recv().await != Ok(Change::Connection { connected: true }) {} };
    tokio::time::timeout(WAIT, back).await.expect("reconnected");
    assert_eq!(
        *asked.lock().unwrap(),
        vec![(UNSERVED, None), (HEADLESS, Some(TermPos(4)))]
    );
}
