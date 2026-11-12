//! One client connection: read a line, answer it, and keep any subscriptions it opened flowing.
//!
//! Requests are answered one at a time in the order they arrive, so a client that sends two
//! without waiting sees their effects in that order. Everything a connection writes goes through
//! one bounded queue. A response waits for room. A notification does not: if the client reads so
//! slowly that the queue is full, its subscription is cut off with `lagged`, and the client
//! subscribes again from its cursor. Memory per connection is therefore bounded.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use x100_proto::{
    BadLine, Call, ClosedBecause, ErrorCode, EventNotice, Hello, Message, Notice, PROTOCOL_VERSION,
    Request, Response, RpcError, Subscribe, SubscribeTerminal, SubscribeTranscript, Subscribed,
    SubscriptionClosed, SubscriptionId, TerminalNotice, TerminalSubscribed, TranscriptNotice,
    TranscriptSubscribed, Unsubscribe, Welcome,
};

use crate::hub::{Hub, NewProject, Registered};
use crate::methods::{self, blocking, store_error};

/// Room kept in the queue for responses and the notice that a subscription lagged, which a full
/// queue of notifications must not crowd out.
const RESERVE: usize = 16;

type Active = Arc<Mutex<HashSet<SubscriptionId>>>;

/// A message in a connection's queue. A notification holds a permit, returned when it has been
/// written, so a client that reads nothing runs the permits out.
struct Queued {
    message: Message,
    _permit: Option<OwnedSemaphorePermit>,
}

#[derive(Clone)]
struct Outbox {
    queue: mpsc::Sender<Queued>,
    permits: Arc<Semaphore>,
    lag_after: Duration,
}

impl Outbox {
    async fn response(&self, response: Response) {
        let message = Message::Response(response);
        let _ = self
            .queue
            .send(Queued {
                message,
                _permit: None,
            })
            .await;
    }

    /// Queues a notification, waiting for room for at most `lag_after`. False if there was none:
    /// the client is not keeping up.
    async fn notify(&self, notice: Notice) -> Result<(), ()> {
        let permit =
            tokio::time::timeout(self.lag_after, Arc::clone(&self.permits).acquire_owned())
                .await
                .map_err(drop)?
                .map_err(drop)?;
        let message = Message::Notice(notice);
        self.queue
            .send(Queued {
                message,
                _permit: Some(permit),
            })
            .await
            .map_err(drop)
    }

    /// For a notice that must get through a full queue: it uses the room kept for it.
    fn urgent(&self, notice: Notice) {
        let message = Message::Notice(notice);
        let _ = self.queue.try_send(Queued {
            message,
            _permit: None,
        });
    }
}

struct Conn {
    hub: Arc<Hub>,
    out: Outbox,
    active: Active,
    subs: HashMap<SubscriptionId, JoinHandle<()>>,
    /// Open gates of subscriptions registered by the request being answered. Opened once its
    /// response is queued, so a notification never comes before the result that names it.
    gates: Vec<watch::Sender<bool>>,
    next_sub: u64,
    said_hello: bool,
    events: Option<SubscriptionId>,
}

pub(crate) async fn serve(hub: Arc<Hub>, stream: UnixStream, mut shutdown: watch::Receiver<bool>) {
    let (read, write) = stream.into_split();
    let (queue_tx, queue) = mpsc::channel(hub.config.queue_capacity + RESERVE);
    let out = Outbox {
        queue: queue_tx,
        permits: Arc::new(Semaphore::new(hub.config.queue_capacity)),
        lag_after: hub.config.lag_after,
    };
    let active: Active = Arc::default();
    let writer = tokio::spawn(write_loop(write, queue, Arc::clone(&active)));
    let max_line = hub.config.max_line;
    let mut conn = Conn {
        hub,
        out,
        active,
        subs: HashMap::new(),
        gates: Vec::new(),
        next_sub: 0,
        said_hello: false,
        events: None,
    };
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    let mut refused = false;
    loop {
        let line = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                conn.close_all(ClosedBecause::Shutdown);
                break;
            }
            line = read_line(&mut reader, &mut buf, max_line) => line,
        };
        match line {
            Ok(Read::Line) => conn.handle(&buf).await,
            Ok(Read::TooLong) => {
                // The next line cannot be found, so the connection ends after the refusal.
                let error = RpcError::new(
                    ErrorCode::TooLarge,
                    format!("a line is at most {max_line} bytes"),
                );
                conn.respond(Response::err(None, error)).await;
                refused = true;
                break;
            }
            Ok(Read::Eof) | Err(_) => break,
        }
    }
    for (_, task) in conn.subs.drain() {
        task.abort();
    }
    drop(conn);
    // Let the writer finish what is queued; a client that never reads must not hold us.
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    if refused {
        // Closing with the rest of the long line unread would reset the connection and could
        // discard the refusal before the client reads it.
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            tokio::io::copy(&mut reader, &mut tokio::io::sink()),
        )
        .await;
    }
}

enum Read {
    Line,
    Eof,
    TooLong,
}

/// One line into `buf`, without its newline. A line over `max` is refused before it is all read.
async fn read_line<R: AsyncBufRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<Read> {
    buf.clear();
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(Read::Eof);
        }
        let end = chunk.iter().position(|b| *b == b'\n');
        let take = end.unwrap_or(chunk.len());
        if buf.len() + take > max {
            return Ok(Read::TooLong);
        }
        buf.extend_from_slice(&chunk[..take]);
        r.consume(take + usize::from(end.is_some()));
        if end.is_some() {
            return Ok(Read::Line);
        }
    }
}

async fn write_loop(mut write: OwnedWriteHalf, mut queue: mpsc::Receiver<Queued>, active: Active) {
    while let Some(Queued { message, _permit }) = queue.recv().await {
        if let Message::Notice(
            Notice::Event(EventNotice::Daemon { subscription, .. })
            | Notice::Event(EventNotice::Project { subscription, .. })
            | Notice::TranscriptEntry(TranscriptNotice { subscription, .. })
            | Notice::TerminalChunk(TerminalNotice { subscription, .. }),
        ) = &message
            && !active.lock().expect("active lock").contains(subscription)
        {
            // Queued before the subscription ended or lagged: it must not follow the notice that
            // said so.
            continue;
        }
        let mut line = message.to_line();
        line.push('\n');
        if write.write_all(line.as_bytes()).await.is_err() {
            return;
        }
    }
    let _ = write.shutdown().await;
}

impl Conn {
    async fn respond(&self, response: Response) {
        self.out.response(response).await;
    }

    async fn handle(&mut self, line: &[u8]) {
        let parsed = match std::str::from_utf8(line) {
            Ok(text) => Message::parse(text),
            Err(e) => Err(BadLine {
                id: None,
                error: RpcError::new(ErrorCode::ParseError, e.to_string()),
            }),
        };
        let response = match parsed {
            Ok(Message::Request(Request { id, call })) => {
                let outcome = self.request(call).await;
                Response {
                    id: Some(id),
                    outcome,
                }
            }
            Err(BadLine { id, error }) => Response::err(id, error),
            Ok(_) => Response::err(
                None,
                RpcError::new(ErrorCode::InvalidRequest, "a client sends requests only"),
            ),
        };
        self.respond(response).await;
        // The result is queued ahead of anything the new subscription sends.
        for gate in self.gates.drain(..) {
            let _ = gate.send(true);
        }
    }

    async fn request(&mut self, call: Call) -> Result<Value, RpcError> {
        if !self.said_hello {
            return match call {
                Call::Hello(h) => self.hello(h),
                _ => Err(RpcError::new(
                    ErrorCode::InvalidRequest,
                    "the first request must be hello",
                )),
            };
        }
        match call {
            Call::Hello(_) => Err(RpcError::new(
                ErrorCode::InvalidRequest,
                "hello was said already",
            )),
            Call::Subscribe(p) => self.subscribe(p).await,
            Call::Unsubscribe(p) => self.unsubscribe(p),
            Call::SubscribeTranscript(p) => self.subscribe_transcript(p).await,
            Call::SubscribeTerminal(p) => self.subscribe_terminal(p).await,
            other => methods::call(&self.hub, other).await,
        }
    }

    fn hello(&mut self, h: Hello) -> Result<Value, RpcError> {
        if h.protocol < PROTOCOL_VERSION {
            return Err(RpcError::new(
                ErrorCode::UnsupportedProtocol,
                format!(
                    "this daemon speaks protocol {PROTOCOL_VERSION}, not {}",
                    h.protocol
                ),
            ));
        }
        self.said_hello = true;
        let welcome = Welcome {
            protocol: PROTOCOL_VERSION,
            daemon: env!("CARGO_PKG_VERSION").to_owned(),
        };
        Ok(serde_json::to_value(welcome).expect("results serialise"))
    }

    fn new_subscription(&mut self) -> (SubscriptionId, watch::Receiver<bool>) {
        self.next_sub += 1;
        let id = SubscriptionId(self.next_sub);
        self.active.lock().expect("active lock").insert(id);
        let (gate, open) = watch::channel(false);
        self.gates.push(gate);
        (id, open)
    }

    async fn subscribe(&mut self, p: Subscribe) -> Result<Value, RpcError> {
        if let Some(old) = self.events {
            if self.active.lock().expect("active lock").contains(&old) {
                return Err(RpcError::new(
                    ErrorCode::Rejected,
                    "this connection follows the logs already; unsubscribe first",
                ));
            }
            // Cut off for lagging: its task is finished with, and this replaces it.
            if let Some(task) = self.subs.remove(&old) {
                task.abort();
            }
        }
        let hub = Arc::clone(&self.hub);
        let mut registered = blocking(move || hub.register(p.resume)).await?;
        let snapshot = registered.snapshot.take();
        let (id, gate) = self.new_subscription();
        self.events = Some(id);
        let task = tokio::spawn(follow_logs(
            registered,
            id,
            gate,
            self.out.clone(),
            Arc::clone(&self.active),
        ));
        self.subs.insert(id, task);
        Ok(serde_json::to_value(Subscribed {
            subscription: id,
            snapshot,
        })
        .expect("results serialise"))
    }

    fn unsubscribe(&mut self, p: Unsubscribe) -> Result<Value, RpcError> {
        let task = self.subs.remove(&p.subscription).ok_or_else(|| {
            RpcError::new(
                ErrorCode::UnknownSubscription,
                "no such subscription on this connection",
            )
        })?;
        task.abort();
        self.active
            .lock()
            .expect("active lock")
            .remove(&p.subscription);
        if self.events == Some(p.subscription) {
            self.events = None;
        }
        Ok(serde_json::json!({}))
    }

    async fn subscribe_transcript(&mut self, p: SubscribeTranscript) -> Result<Value, RpcError> {
        let store = methods::project(&self.hub, p.project)?;
        if store.log.with_state(|s| s.agent(p.agent).is_none()) {
            return Err(RpcError::new(
                ErrorCode::NotFound,
                format!("no agent {}", p.agent),
            ));
        }
        let (agent, after) = (p.agent, p.after);
        let mut entries = blocking(move || store.transcripts.follow(agent, after))
            .await
            .map_err(|e| store_error(&e))?;
        let (id, gate) = self.new_subscription();
        let (out, active) = (self.out.clone(), Arc::clone(&self.active));
        let task = tokio::spawn(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(item) = entries.recv().await {
                    if tx.send(item).is_err() {
                        return;
                    }
                }
            });
            forward(rx, gate, id, out, active, move |(position, entry)| {
                Notice::TranscriptEntry(TranscriptNotice {
                    subscription: id,
                    position,
                    entry,
                })
            })
            .await;
        });
        self.subs.insert(id, task);
        Ok(
            serde_json::to_value(TranscriptSubscribed { subscription: id })
                .expect("results serialise"),
        )
    }

    /// Follows a terminal driver's terminal as `subscribe_transcript` follows a transcript: the
    /// store replays and then follows under one lock, so no chunk is missed or repeated.
    async fn subscribe_terminal(&mut self, p: SubscribeTerminal) -> Result<Value, RpcError> {
        let store = methods::terminal_of(&self.hub, p.project, p.driver)?;
        let (driver, after) = (p.driver, p.after);
        let chunks = blocking(move || store.terminals.follow(driver, after))
            .await
            .map_err(|e| store_error(&e))?;
        let (id, gate) = self.new_subscription();
        let (out, active) = (self.out.clone(), Arc::clone(&self.active));
        let task = tokio::spawn(forward(
            chunks,
            gate,
            id,
            out,
            active,
            move |(position, chunk)| {
                Notice::TerminalChunk(TerminalNotice {
                    subscription: id,
                    position,
                    chunk,
                })
            },
        ));
        self.subs.insert(id, task);
        Ok(
            serde_json::to_value(TerminalSubscribed { subscription: id })
                .expect("results serialise"),
        )
    }

    /// The daemon is stopping: tell every subscription, so its client does not wait for more.
    fn close_all(&mut self, because: ClosedBecause) {
        let ids: Vec<_> = self.active.lock().expect("active lock").drain().collect();
        for subscription in ids {
            let notice = Notice::Closed(SubscriptionClosed {
                subscription,
                because,
            });
            self.out.urgent(notice);
        }
    }
}

/// Follows the daemon's log and every project's, including projects opened later, for one
/// subscription. Each log has a task of its own, all stopped when this one is.
async fn follow_logs(
    registered: Registered,
    sub: SubscriptionId,
    gate: watch::Receiver<bool>,
    out: Outbox,
    active: Active,
) {
    let Registered {
        daemon,
        projects,
        mut new_projects,
        ..
    } = registered;
    let mut tasks = JoinSet::new();
    let ctx = |gate: &watch::Receiver<bool>| (gate.clone(), out.clone(), Arc::clone(&active));
    let (g, o, a) = ctx(&gate);
    tasks.spawn(forward(daemon, g, sub, o, a, move |envelope| {
        Notice::Event(EventNotice::Daemon {
            subscription: sub,
            envelope,
        })
    }));
    for (project, rx) in projects {
        let (g, o, a) = ctx(&gate);
        tasks.spawn(forward(rx, g, sub, o, a, move |envelope| {
            Notice::Event(EventNotice::Project {
                subscription: sub,
                project,
                envelope,
            })
        }));
    }
    while let Some(NewProject { id: project, store }) = new_projects.recv().await {
        // From the first event, whatever has been appended since the project was opened.
        let Ok(rx) = blocking(move || store.log.follow_after(None)).await else {
            continue;
        };
        let (g, o, a) = ctx(&gate);
        tasks.spawn(forward(rx, g, sub, o, a, move |envelope| {
            Notice::Event(EventNotice::Project {
                subscription: sub,
                project,
                envelope,
            })
        }));
    }
}

/// Moves what a store follower yields into the connection's queue, once the subscription's result
/// has been queued. If the client leaves the queue full for `lag_after`, the subscription ends as
/// lagged. A client replaying a long backlog is reading, so it is never cut off for the size of it.
async fn forward<T>(
    mut rx: mpsc::UnboundedReceiver<T>,
    mut gate: watch::Receiver<bool>,
    sub: SubscriptionId,
    out: Outbox,
    active: Active,
    make: impl Fn(T) -> Notice,
) {
    if gate.wait_for(|open| *open).await.is_err() {
        return;
    }
    while let Some(item) = rx.recv().await {
        if !active.lock().expect("active lock").contains(&sub) {
            return;
        }
        if out.notify(make(item)).await.is_err() {
            // Whichever log notices first says so, once.
            if active.lock().expect("active lock").remove(&sub) {
                out.urgent(Notice::Closed(SubscriptionClosed {
                    subscription: sub,
                    because: ClosedBecause::Lagged,
                }));
            }
            return;
        }
    }
}
