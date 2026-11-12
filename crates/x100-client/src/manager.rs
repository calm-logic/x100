//! The task that owns the connection: connects, takes the snapshot, applies the stream, answers
//! requests, and starts again from the cursors when the connection drops.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::{mpsc, oneshot};
use x100_proto::{
    Call, ClosedBecause, ErrorCode, EventNotice, Hello, Id, Message, Method, Notice,
    PROTOCOL_VERSION, Request, Response, Subscribe, SubscribeTerminal, SubscribeTranscript,
    Subscribed, SubscriptionId, Welcome,
};

use crate::{Change, ClientError, Cmd, Options, Shared, Target};

type Reply = oneshot::Sender<Result<Value, ClientError>>;

struct Manager {
    options: Options,
    shared: std::sync::Arc<Shared>,
    cmds: mpsc::UnboundedReceiver<Cmd>,
    followers: HashMap<u64, Target>,
    next_follower: u64,
}

/// One connection.
struct Conn {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: mpsc::UnboundedSender<String>,
    pending: HashMap<u64, Reply>,
    next_id: u64,
    events: Option<SubscriptionId>,
    /// Which follower each transcript or terminal subscription feeds.
    follows: HashMap<SubscriptionId, u64>,
    /// An event could not be applied: the state is wrong and a new snapshot must replace it.
    resync: bool,
    /// The daemon ended the event subscription because this client read too slowly.
    lagged: bool,
    /// The daemon is stopping.
    shutdown: bool,
    lagged_follows: Vec<u64>,
}

enum End {
    Closed,
    Lost,
}

pub(crate) async fn run(
    options: Options,
    shared: std::sync::Arc<Shared>,
    cmds: mpsc::UnboundedReceiver<Cmd>,
    ready: oneshot::Sender<Result<(), ClientError>>,
) {
    let mut m = Manager {
        options,
        shared,
        cmds,
        followers: HashMap::new(),
        next_follower: 0,
    };
    let mut ready = Some(ready);
    loop {
        let Some(stream) = m.connect(&mut ready).await else {
            if let Some(r) = ready.take() {
                let _ = r.send(Err(ClientError::Closed));
            }
            return;
        };
        match m.session(stream, &mut ready).await {
            End::Closed => return,
            End::Lost => m.announce(Change::Connection { connected: false }),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

impl Manager {
    fn announce(&self, change: Change) {
        let _ = self.shared.changes.send(change);
    }

    async fn connect(
        &mut self,
        ready: &mut Option<oneshot::Sender<Result<(), ClientError>>>,
    ) -> Option<UnixStream> {
        let started = Instant::now();
        let mut backoff = Duration::from_millis(25);
        let mut launched: Option<Instant> = None;
        loop {
            match UnixStream::connect(&self.options.socket).await {
                Ok(stream) => return Some(stream),
                Err(e) => {
                    use std::io::ErrorKind::{ConnectionRefused, NotFound};
                    let nothing_there = matches!(e.kind(), NotFound | ConnectionRefused);
                    if nothing_there
                        && let Some(launch) = &self.options.launch
                        && launched.is_none_or(|t| t.elapsed() > Duration::from_secs(3))
                    {
                        launched = Some(Instant::now());
                        // A process group of its own, so the client's Ctrl-C and exit leave it be.
                        let spawned = tokio::process::Command::new(&launch.program)
                            .args(&launch.args)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .process_group(0)
                            .spawn();
                        if let Err(e) = spawned {
                            if let Some(r) = ready.take() {
                                let _ = r.send(Err(ClientError::NoDaemon(format!(
                                    "could not start {}: {e}",
                                    launch.program.display()
                                ))));
                            }
                            return None;
                        }
                    }
                    if ready.is_some() && started.elapsed() > self.options.startup_timeout {
                        if let Some(r) = ready.take() {
                            let _ = r.send(Err(ClientError::NoDaemon(e.to_string())));
                        }
                        return None;
                    }
                }
            }
            let nap = tokio::time::sleep(backoff);
            tokio::pin!(nap);
            loop {
                tokio::select! {
                    () = &mut nap => break,
                    cmd = self.cmds.recv() => match cmd {
                        None | Some(Cmd::Close) => return None,
                        Some(cmd) => self.offline(cmd),
                    },
                }
            }
            backoff = (backoff * 2).min(Duration::from_millis(500));
        }
    }

    /// A command while there is no connection. Following is remembered and starts when the
    /// connection does; a request is refused at once rather than left to wait.
    fn offline(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Request(_, reply) => {
                let _ = reply.send(Err(ClientError::Disconnected));
            }
            Cmd::Follow { target, reply } => {
                let id = self.add_follower(target);
                let _ = reply.send(Ok(id));
            }
            Cmd::Unfollow(id) => {
                self.followers.remove(&id);
            }
            Cmd::Reconnect | Cmd::Close => {}
        }
    }

    fn add_follower(&mut self, target: Target) -> u64 {
        self.next_follower += 1;
        self.followers.insert(self.next_follower, target);
        self.next_follower
    }

    async fn session(
        &mut self,
        stream: UnixStream,
        ready: &mut Option<oneshot::Sender<Result<(), ClientError>>>,
    ) -> End {
        let (read, mut write) = stream.into_split();
        // Writing has a task of its own: if the daemon is writing to us while we are writing to
        // it, neither side may wait for the other to read.
        let (writer, mut to_write) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(mut line) = to_write.recv().await {
                line.push('\n');
                if write.write_all(line.as_bytes()).await.is_err() {
                    return;
                }
            }
        });
        let mut conn = Conn {
            lines: BufReader::new(read).lines(),
            writer,
            pending: HashMap::new(),
            next_id: 0,
            events: None,
            follows: HashMap::new(),
            resync: false,
            lagged: false,
            shutdown: false,
            lagged_follows: Vec::new(),
        };
        if let Err(e) = self.handshake(&mut conn).await {
            if let Some(r) = ready.take() {
                let _ = r.send(Err(e));
                return End::Closed;
            }
            return End::Lost;
        }
        if let Some(r) = ready.take() {
            let _ = r.send(Ok(()));
        }
        self.announce(Change::Connection { connected: true });
        loop {
            tokio::select! {
                line = conn.lines.next_line() => match line {
                    Ok(Some(line)) => self.on_line(&mut conn, &line),
                    _ => return conn.lose(),
                },
                cmd = self.cmds.recv() => match cmd {
                    None | Some(Cmd::Close) => return End::Closed,
                    Some(Cmd::Reconnect) => return conn.lose(),
                    Some(Cmd::Follow { target, reply }) => {
                        let id = self.add_follower(target);
                        let subscribed = self.subscribe_follower(&mut conn, id).await;
                        let _ = reply.send(match subscribed {
                            Ok(()) if self.followers.contains_key(&id) => Ok(id),
                            Ok(()) => Err(ClientError::Rpc(x100_proto::RpcError::new(
                                ErrorCode::NotFound,
                                "nothing to follow there",
                            ))),
                            Err(e) => Err(e),
                        });
                    }
                    Some(cmd) => self.on_command(&mut conn, cmd),
                },
            }
            if conn.shutdown {
                return conn.lose();
            }
            if conn.resync {
                // Nothing already held can be trusted: start from a snapshot.
                self.shared.have_snapshot.store(false, Ordering::SeqCst);
                return conn.lose();
            }
            if let Err(e) = self.catch_up(&mut conn).await {
                if matches!(e, ClientError::Closed) {
                    return End::Closed;
                }
                return conn.lose();
            }
        }
    }

    /// Subscribes again where a subscription was cut off for lagging.
    async fn catch_up(&mut self, conn: &mut Conn) -> Result<(), ClientError> {
        if conn.lagged {
            conn.lagged = false;
            self.subscribe_events(conn).await?;
        }
        for id in std::mem::take(&mut conn.lagged_follows) {
            self.resubscribe(conn, id).await?;
        }
        Ok(())
    }

    async fn handshake(&mut self, conn: &mut Conn) -> Result<(), ClientError> {
        let hello = Hello {
            client: self.options.name.clone(),
            protocol: PROTOCOL_VERSION,
        };
        let welcome: Welcome = self.rpc(conn, hello).await?;
        if welcome.protocol != PROTOCOL_VERSION {
            return Err(ClientError::Protocol(format!(
                "the daemon speaks protocol {}",
                welcome.protocol
            )));
        }
        self.subscribe_events(conn).await?;
        let ids: Vec<u64> = self.followers.keys().copied().collect();
        for id in ids {
            self.resubscribe(conn, id).await?;
        }
        Ok(())
    }

    /// Resumes from the cursors if a snapshot is held, and takes a snapshot if not, or if the
    /// daemon says the cursors are not its own.
    async fn subscribe_events(&mut self, conn: &mut Conn) -> Result<(), ClientError> {
        let have = self.shared.have_snapshot.load(Ordering::SeqCst);
        let resume = have.then(|| self.shared.state.read().expect("state lock").cursor());
        let answer = match self.rpc(conn, Subscribe { resume }).await {
            Err(ClientError::Rpc(e)) if have && e.code == ErrorCode::Rejected => {
                self.rpc(conn, Subscribe { resume: None }).await?
            }
            other => other?,
        };
        let Subscribed {
            subscription,
            snapshot,
        } = answer;
        conn.events = Some(subscription);
        if let Some(snapshot) = snapshot {
            *self.shared.state.write().expect("state lock") = snapshot;
            self.shared.have_snapshot.store(true, Ordering::SeqCst);
            self.announce(Change::Reset);
        }
        Ok(())
    }

    /// Follows again after a reconnect or a lag. A refusal has already ended that stream, and
    /// is no reason to drop the connection.
    async fn resubscribe(&mut self, conn: &mut Conn, id: u64) -> Result<(), ClientError> {
        match self.subscribe_follower(conn, id).await {
            Err(ClientError::Rpc(_)) => Ok(()),
            other => other,
        }
    }

    async fn subscribe_follower(&mut self, conn: &mut Conn, id: u64) -> Result<(), ClientError> {
        conn.follows.retain(|_, local| *local != id);
        let subscribed = match self.followers.get(&id) {
            None => return Ok(()),
            Some(Target::Transcript {
                project,
                agent,
                last,
                ..
            }) => {
                let request = SubscribeTranscript {
                    project: *project,
                    agent: *agent,
                    after: *last,
                };
                self.rpc(conn, request).await.map(|done| done.subscription)
            }
            Some(Target::Terminal {
                project,
                driver,
                last,
                ..
            }) => {
                let request = SubscribeTerminal {
                    project: *project,
                    driver: *driver,
                    after: *last,
                };
                self.rpc(conn, request).await.map(|done| done.subscription)
            }
        };
        match subscribed {
            Ok(subscription) => {
                conn.follows.insert(subscription, id);
                Ok(())
            }
            // The agent or the terminal is gone, so nothing will come: end its stream.
            Err(ClientError::Rpc(e)) if e.code == ErrorCode::NotFound => {
                self.followers.remove(&id);
                Ok(())
            }
            // Refused for another reason, such as a daemon that cannot serve it yet: the stream
            // ends too, and the caller of a new follow is told why.
            Err(ClientError::Rpc(e)) => {
                self.followers.remove(&id);
                Err(ClientError::Rpc(e))
            }
            Err(e) => Err(e),
        }
    }

    /// A request made during the handshake: sent, then lines are read and applied until its
    /// answer comes.
    async fn rpc<M: Method + Into<Call>>(
        &mut self,
        conn: &mut Conn,
        params: M,
    ) -> Result<M::Result, ClientError> {
        let (tx, mut rx) = oneshot::channel();
        conn.send(params.into(), tx);
        let value = loop {
            // The answer first: notices for a subscription follow its result closely, and must not
            // be read before the caller has recorded the subscription they belong to.
            tokio::select! {
                biased;
                answer = &mut rx => break answer.map_err(|_| ClientError::Disconnected)??,
                line = conn.lines.next_line() => match line {
                    Ok(Some(line)) => self.on_line(conn, &line),
                    _ => return Err(ClientError::Disconnected),
                },
            }
        };
        serde_json::from_value(value)
            .map_err(|e| ClientError::Protocol(format!("a malformed result of {}: {e}", M::NAME)))
    }

    fn on_command(&mut self, conn: &mut Conn, cmd: Cmd) {
        match cmd {
            Cmd::Request(call, reply) => conn.send(call, reply),
            Cmd::Follow { .. } => unreachable!("the session loop follows"),
            Cmd::Unfollow(id) => {
                self.followers.remove(&id);
                if let Some((sub, _)) = conn.follows.iter().find(|(_, l)| **l == id) {
                    let sub = *sub;
                    conn.follows.remove(&sub);
                    let (tx, _) = oneshot::channel();
                    conn.send(x100_proto::Unsubscribe { subscription: sub }.into(), tx);
                }
            }
            Cmd::Reconnect | Cmd::Close => {}
        }
    }

    fn on_line(&mut self, conn: &mut Conn, line: &str) {
        match Message::parse(line) {
            Ok(Message::Response(Response {
                id: Some(Id::Number(id)),
                outcome,
            })) => {
                if let Some(reply) = conn.pending.remove(&id) {
                    let _ = reply.send(outcome.map_err(ClientError::Rpc));
                }
            }
            Ok(Message::Notice(notice)) => self.on_notice(conn, notice),
            // A line this client cannot read is a daemon it cannot trust.
            Err(_) | Ok(_) => conn.resync = true,
        }
    }

    fn on_notice(&mut self, conn: &mut Conn, notice: Notice) {
        match notice {
            Notice::Event(event) => {
                let (sub, change) = {
                    let mut state = self.shared.state.write().expect("state lock");
                    match event {
                        EventNotice::Daemon {
                            subscription,
                            envelope,
                        } => {
                            if Some(subscription) != conn.events {
                                return;
                            }
                            (
                                subscription,
                                state.daemon.apply(&envelope).map(Change::Daemon),
                            )
                        }
                        EventNotice::Project {
                            subscription,
                            project,
                            envelope,
                        } => {
                            if Some(subscription) != conn.events {
                                return;
                            }
                            // A project opened since the snapshot starts empty and is told its
                            // first event.
                            let fresh = !state.projects.contains_key(&project);
                            let result =
                                state.projects.entry(project).or_default().apply(&envelope);
                            if result.is_err() && fresh {
                                state.projects.remove(&project);
                            }
                            (
                                subscription,
                                result.map(|changes| Change::Project { project, changes }),
                            )
                        }
                    }
                };
                let _ = sub;
                match change {
                    Ok(change) => self.announce(change),
                    // An event missed or repeated: the state is no longer the daemon's.
                    Err(_) => conn.resync = true,
                }
            }
            Notice::TranscriptEntry(t) => {
                let local = conn.follows.get(&t.subscription);
                if let Some(Target::Transcript { last, items, .. }) =
                    local.and_then(|l| self.followers.get_mut(l))
                    && last.is_none_or(|last| t.position > last)
                {
                    *last = Some(t.position);
                    let _ = items.send((t.position, t.entry));
                }
            }
            Notice::TerminalChunk(t) => {
                let local = conn.follows.get(&t.subscription);
                if let Some(Target::Terminal { last, items, .. }) =
                    local.and_then(|l| self.followers.get_mut(l))
                    && last.is_none_or(|last| t.position > last)
                {
                    *last = Some(t.position);
                    let _ = items.send((t.position, t.chunk));
                }
            }
            Notice::Closed(closed) => {
                if Some(closed.subscription) == conn.events {
                    conn.events = None;
                    match closed.because {
                        ClosedBecause::Lagged => conn.lagged = true,
                        ClosedBecause::Shutdown => conn.shutdown = true,
                    }
                } else if let Some(local) = conn.follows.remove(&closed.subscription) {
                    match closed.because {
                        ClosedBecause::Lagged => conn.lagged_follows.push(local),
                        ClosedBecause::Shutdown => conn.shutdown = true,
                    }
                }
            }
        }
    }
}

impl Conn {
    fn send(&mut self, call: Call, reply: Reply) {
        self.next_id += 1;
        let id = self.next_id;
        let line = Message::Request(Request {
            id: Id::Number(id),
            call,
        })
        .to_line();
        self.pending.insert(id, reply);
        let _ = self.writer.send(line);
    }

    /// The connection is over: whoever waits for an answer is told so.
    fn lose(&mut self) -> End {
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err(ClientError::Disconnected));
        }
        End::Lost
    }
}
