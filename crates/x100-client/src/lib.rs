//! What every interface shares: the connection to the daemon, and the state each tab draws from.
//!
//! A [`Client`] connects to the daemon's socket (starting the daemon if nothing answers and it was
//! told how), says hello, takes a snapshot of every log, and then applies each event to that
//! snapshot with core's own `apply`, so it holds exactly what the daemon's folds hold. It keeps
//! the connection: when it drops, the client connects again and resumes from the sequence number
//! of each log it had reached, so no event is lost or repeated. What changed is announced on a
//! [`Change`] stream, in core's [`Changes`] terms, so an interface redraws only that.
//!
//! The client holds no logic beyond that. Requests are the protocol's own typed params.

mod manager;
mod stream;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use x100_core::fold::{Changes, DaemonPart, ProjectPart, ProjectState};
use x100_core::id::{AgentId, ProjectId};
use x100_core::transcript::Pos;
use x100_proto::{Call, Method, RpcError, Snapshot};

pub use stream::TranscriptStream;

/// How to start the daemon when nothing answers on the socket.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub socket: PathBuf,
    /// Who is connecting, for the daemon's log.
    pub name: String,
    /// Run when nothing answers. The daemon outlives the client that started it.
    pub launch: Option<Launch>,
    /// How long [`Client::connect`] tries before it gives up.
    pub startup_timeout: Duration,
}

impl Options {
    pub fn new(socket: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
            name: name.into(),
            launch: None,
            startup_timeout: Duration::from_secs(10),
        }
    }
}

/// `$X100_DATA_DIR/x100d.sock`, or under `$XDG_DATA_HOME/x100`, or `~/.local/share/x100`: the
/// socket of a daemon started without a data directory of its own.
pub fn default_socket() -> Option<PathBuf> {
    let dir = std::env::var_os("X100_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let base = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
                })?;
            Some(base.join("x100"))
        })?;
    Some(dir.join("x100d.sock"))
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The daemon refused the request.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// The connection was lost before the answer came, or is not up. The request may or may not
    /// have been carried out.
    #[error("the connection to the daemon is down")]
    Disconnected,
    /// The client was closed.
    #[error("the client is closed")]
    Closed,
    #[error("no daemon answered: {0}")]
    NoDaemon(String),
    /// The daemon said something this client does not understand.
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("timed out")]
    Timeout,
}

/// What changed in the held state. After `Reset` everything may have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The state was replaced by a new snapshot.
    Reset,
    Connection {
        connected: bool,
    },
    Daemon(Changes<DaemonPart>),
    Project {
        project: ProjectId,
        changes: Changes<ProjectPart>,
    },
}

pub(crate) struct Shared {
    pub state: RwLock<Snapshot>,
    /// Whether `state` has ever been filled by a snapshot, so a reconnect knows to resume.
    pub have_snapshot: std::sync::atomic::AtomicBool,
    pub changes: broadcast::Sender<Change>,
}

pub(crate) enum Cmd {
    Request(
        Call,
        oneshot::Sender<Result<serde_json::Value, ClientError>>,
    ),
    Follow {
        project: ProjectId,
        agent: AgentId,
        after: Option<Pos>,
        entries: mpsc::UnboundedSender<(Pos, x100_core::transcript::Entry)>,
        reply: oneshot::Sender<Result<u64, ClientError>>,
    },
    Unfollow(u64),
    Reconnect,
    Close,
}

struct Inner {
    shared: Arc<Shared>,
    cmds: mpsc::UnboundedSender<Cmd>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Close);
    }
}

/// A connection to the daemon and the state folded from its logs. Cloning shares both; the
/// connection closes with the last clone.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Connects, and returns once the first snapshot is held.
    pub async fn connect(options: Options) -> Result<Client, ClientError> {
        let (changes, _) = broadcast::channel(4096);
        let shared = Arc::new(Shared {
            state: RwLock::new(Snapshot::default()),
            have_snapshot: false.into(),
            changes,
        });
        let (cmds, rx) = mpsc::unbounded_channel();
        let (ready_tx, ready) = oneshot::channel();
        tokio::spawn(manager::run(options, Arc::clone(&shared), rx, ready_tx));
        ready.await.map_err(|_| ClientError::Closed)??;
        Ok(Client {
            inner: Arc::new(Inner { shared, cmds }),
        })
    }

    /// Sends a request and waits for its typed result.
    pub async fn request<M: Method + Into<Call>>(
        &self,
        params: M,
    ) -> Result<M::Result, ClientError> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .cmds
            .send(Cmd::Request(params.into(), tx))
            .map_err(|_| ClientError::Closed)?;
        let value = rx.await.map_err(|_| ClientError::Closed)??;
        serde_json::from_value(value)
            .map_err(|e| ClientError::Protocol(format!("a malformed result of {}: {e}", M::NAME)))
    }

    /// Reads the held state: the daemon's and every project's.
    pub fn with_state<R>(&self, f: impl FnOnce(&Snapshot) -> R) -> R {
        f(&self.inner.shared.state.read().expect("state lock"))
    }

    pub fn state(&self) -> Snapshot {
        self.with_state(Snapshot::clone)
    }

    pub fn project(&self, id: ProjectId) -> Option<ProjectState> {
        self.with_state(|s| s.projects.get(&id).cloned())
    }

    /// What changes from now on. A receiver that falls too far behind gets `Lagged` and should
    /// redraw everything.
    pub fn changes(&self) -> broadcast::Receiver<Change> {
        self.inner.shared.changes.subscribe()
    }

    /// Waits until `f` returns something for the held state, checking after every change.
    pub async fn wait_for<T>(
        &self,
        timeout: Duration,
        mut f: impl FnMut(&Snapshot) -> Option<T>,
    ) -> Result<T, ClientError> {
        let mut changes = self.changes();
        let wait = async {
            loop {
                if let Some(found) = self.with_state(&mut f) {
                    return found;
                }
                match changes.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| ClientError::Timeout)
    }

    /// Follows an agent's transcript from after `after`, across reconnects, each entry once.
    pub async fn follow_transcript(
        &self,
        project: ProjectId,
        agent: AgentId,
        after: Option<Pos>,
    ) -> Result<TranscriptStream, ClientError> {
        let (entries_tx, entries) = mpsc::unbounded_channel();
        let (reply, answer) = oneshot::channel();
        self.inner
            .cmds
            .send(Cmd::Follow {
                project,
                agent,
                after,
                entries: entries_tx,
                reply,
            })
            .map_err(|_| ClientError::Closed)?;
        let id = answer.await.map_err(|_| ClientError::Closed)??;
        Ok(TranscriptStream::new(id, entries, self.inner.cmds.clone()))
    }

    /// Drops the connection. The client connects again by itself and resumes where it was.
    pub fn reconnect(&self) {
        let _ = self.inner.cmds.send(Cmd::Reconnect);
    }
}
