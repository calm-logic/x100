//! The tunnel between a sandbox and its host.
//!
//! A sandbox has no network of its own, and neither harness can reach the daemon or the proxy
//! through a Unix socket. So the helper that anchors the sandbox listens inside on the same
//! loopback address and port as each mapped endpoint has on the host, and carries every
//! connection over its own standard input and output, where the host end connects it to the real
//! address. A URL is then the same inside and out, and the bytes, a `Host` header among them,
//! arrive untouched. The same tunnel serves both backends: a pipe to a process works with
//! bubblewrap and with a container runtime on any platform, where a socket in a shared directory
//! does not cross into a virtual machine.
//!
//! Frames are `stream: u32, kind: u8, length: u32, payload`, big-endian. There is no flow control
//! per stream: data waits in memory until its socket takes it, so one slow reader never holds up
//! the others, at the cost of memory if a reader stops reading.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const READY: u8 = 1;
const LISTEN: u8 = 2;
const LISTENED: u8 = 3;
const OPEN: u8 = 4;
const DATA: u8 = 5;
const EOF: u8 = 6;

/// No frame from either end is near this, so a larger length means the stream is corrupt.
const MAX_FRAME: usize = 1 << 20;
const CHUNK: usize = 64 * 1024;

struct Frame {
    stream: u32,
    kind: u8,
    payload: Vec<u8>,
}

impl Frame {
    fn new(stream: u32, kind: u8, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            stream,
            kind,
            payload: payload.into(),
        }
    }
}

async fn read_frame(from: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Frame>> {
    let mut head = [0u8; 9];
    match from.read_exact(&mut head).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let stream = u32::from_be_bytes(head[0..4].try_into().expect("four bytes"));
    let len = u32::from_be_bytes(head[5..9].try_into().expect("four bytes")) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too long"));
    }
    let mut payload = vec![0u8; len];
    from.read_exact(&mut payload).await?;
    Ok(Some(Frame {
        stream,
        kind: head[4],
        payload,
    }))
}

type Inbox = mpsc::UnboundedSender<Option<Vec<u8>>>;

/// Both ends' shared half: one writer of frames, and the streams open through it.
#[derive(Clone)]
struct Mux {
    out: mpsc::UnboundedSender<Frame>,
    streams: Arc<Mutex<HashMap<u32, Inbox>>>,
}

impl Mux {
    fn new(to: impl AsyncWrite + Unpin + Send + 'static) -> (Self, JoinHandle<()>) {
        let (out, mut frames) = mpsc::unbounded_channel::<Frame>();
        let writer = tokio::spawn(async move {
            let mut to = BufWriter::new(to);
            while let Some(first) = frames.recv().await {
                let mut next = Some(first);
                // Everything already queued goes out in one flush.
                while let Some(f) = next {
                    let len = u32::try_from(f.payload.len()).expect("frames are small");
                    let mut head = [0u8; 9];
                    head[0..4].copy_from_slice(&f.stream.to_be_bytes());
                    head[4] = f.kind;
                    head[5..9].copy_from_slice(&len.to_be_bytes());
                    if to.write_all(&head).await.is_err() || to.write_all(&f.payload).await.is_err()
                    {
                        return;
                    }
                    next = frames.try_recv().ok();
                }
                if to.flush().await.is_err() {
                    return;
                }
            }
        });
        let mux = Self {
            out,
            streams: Arc::default(),
        };
        (mux, writer)
    }

    fn send(&self, frame: Frame) {
        // A closed writer means the other end is gone, which the reader also sees.
        let _ = self.out.send(frame);
    }

    /// Registers `stream` before anything can arrive for it.
    fn register(&self, stream: u32) -> mpsc::UnboundedReceiver<Option<Vec<u8>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.streams.lock().expect("tunnel lock").insert(stream, tx);
        rx
    }

    fn deliver(&self, frame: Frame) {
        let streams = self.streams.lock().expect("tunnel lock");
        if let Some(inbox) = streams.get(&frame.stream) {
            let data = (frame.kind == DATA).then_some(frame.payload);
            let _ = inbox.send(data);
        }
    }

    /// Carries `socket` both ways until each direction has ended.
    async fn pump(
        self,
        stream: u32,
        socket: TcpStream,
        mut inbox: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
    ) {
        let _ = socket.set_nodelay(true);
        let (mut read, mut write) = socket.into_split();
        let reading = async {
            let mut buf = vec![0u8; CHUNK];
            loop {
                match read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => self.send(Frame::new(stream, DATA, &buf[..n])),
                }
            }
            self.send(Frame::new(stream, EOF, []));
        };
        let writing = async {
            while let Some(Some(data)) = inbox.recv().await {
                if write.write_all(&data).await.is_err() {
                    break;
                }
            }
            let _ = write.shutdown().await;
        };
        tokio::join!(reading, writing);
        self.streams.lock().expect("tunnel lock").remove(&stream);
    }
}

/// The end inside the sandbox: runs until the host closes its side.
pub async fn inside(
    from_host: impl AsyncRead + Unpin,
    to_host: impl AsyncWrite + Unpin + Send + 'static,
) -> io::Result<()> {
    let (mux, _writer) = Mux::new(to_host);
    mux.send(Frame::new(0, READY, []));
    let mut from_host = from_host;
    let next_stream = Arc::new(std::sync::atomic::AtomicU32::new(1));
    while let Some(frame) = read_frame(&mut from_host).await? {
        match frame.kind {
            LISTEN => {
                let answer = match listen(&frame.payload).await {
                    Ok(listener) => {
                        let port = listener.local_addr()?.port();
                        tokio::spawn(accept(
                            listener,
                            port,
                            mux.clone(),
                            Arc::clone(&next_stream),
                        ));
                        String::new()
                    }
                    Err(e) => e.to_string(),
                };
                mux.send(Frame::new(frame.stream, LISTENED, answer));
            }
            DATA | EOF => mux.deliver(frame),
            _ => {}
        }
    }
    Ok(())
}

async fn listen(payload: &[u8]) -> io::Result<TcpListener> {
    let addr: SocketAddr = std::str::from_utf8(payload)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an address"))?;
    TcpListener::bind(addr).await
}

async fn accept(
    listener: TcpListener,
    port: u16,
    mux: Mux,
    next: Arc<std::sync::atomic::AtomicU32>,
) {
    while let Ok((socket, _)) = listener.accept().await {
        let stream = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let inbox = mux.register(stream);
        mux.send(Frame::new(stream, OPEN, port.to_be_bytes()));
        tokio::spawn(mux.clone().pump(stream, socket, inbox));
    }
}

/// The host's end: connects what arrives for each mapped port to its real address.
pub(crate) struct Host {
    mux: Mux,
    routes: Arc<Mutex<HashMap<u16, SocketAddr>>>,
    answers: Arc<Mutex<HashMap<u32, oneshot::Sender<String>>>>,
    next_request: std::sync::atomic::AtomicU32,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Host {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Host {
    /// Starts the host's end on the helper's pipes and waits until the helper says it is ready.
    pub(crate) async fn start(
        from_inside: impl AsyncRead + Unpin + Send + 'static,
        to_inside: impl AsyncWrite + Unpin + Send + 'static,
        wait: Duration,
    ) -> io::Result<Host> {
        let (mux, writer) = Mux::new(to_inside);
        let routes: Arc<Mutex<HashMap<u16, SocketAddr>>> = Arc::default();
        let answers: Arc<Mutex<HashMap<u32, oneshot::Sender<String>>>> = Arc::default();
        let (ready_tx, ready) = oneshot::channel();
        let reader = tokio::spawn({
            let (mux, routes, answers) = (mux.clone(), Arc::clone(&routes), Arc::clone(&answers));
            async move {
                let mut from_inside = from_inside;
                let mut ready_tx = Some(ready_tx);
                while let Ok(Some(frame)) = read_frame(&mut from_inside).await {
                    match frame.kind {
                        READY => {
                            if let Some(tx) = ready_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        LISTENED => {
                            let answer = String::from_utf8_lossy(&frame.payload).into_owned();
                            let waiting =
                                answers.lock().expect("tunnel lock").remove(&frame.stream);
                            if let Some(tx) = waiting {
                                let _ = tx.send(answer);
                            }
                        }
                        OPEN => open(&mux, &routes, frame),
                        DATA | EOF => mux.deliver(frame),
                        _ => {}
                    }
                }
            }
        });
        let host = Host {
            mux,
            routes,
            answers,
            next_request: std::sync::atomic::AtomicU32::new(1),
            tasks: vec![reader, writer],
        };
        match tokio::time::timeout(wait, ready).await {
            Ok(Ok(())) => Ok(host),
            Ok(Err(_)) => Err(io::Error::other(
                "the sandbox's helper ended before it was ready",
            )),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the sandbox's helper did not say it was ready",
            )),
        }
    }

    /// Makes `to`, a loopback address on the host, reachable at the same address inside.
    pub(crate) async fn forward(&self, to: SocketAddr) -> io::Result<()> {
        if !to.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{to} is not a loopback address"),
            ));
        }
        let request = self
            .next_request
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.answers
            .lock()
            .expect("tunnel lock")
            .insert(request, tx);
        self.routes
            .lock()
            .expect("tunnel lock")
            .insert(to.port(), to);
        self.mux
            .send(Frame::new(request, LISTEN, to.to_string().into_bytes()));
        let answer = tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no answer from the sandbox"))?
            .map_err(|_| io::Error::other("the sandbox's helper has ended"))?;
        if answer.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(format!("inside the sandbox: {answer}")))
        }
    }
}

fn open(mux: &Mux, routes: &Mutex<HashMap<u16, SocketAddr>>, frame: Frame) {
    let stream = frame.stream;
    let inbox = mux.register(stream);
    let port = <[u8; 2]>::try_from(frame.payload.as_slice()).map(u16::from_be_bytes);
    let to = port
        .ok()
        .and_then(|p| routes.lock().expect("tunnel lock").get(&p).copied());
    let mux = mux.clone();
    tokio::spawn(async move {
        let socket = match to {
            Some(to) => TcpStream::connect(to).await.ok(),
            None => None,
        };
        match socket {
            Some(socket) => mux.pump(stream, socket, inbox).await,
            None => {
                // Refused: the inside sees its connection closed.
                mux.send(Frame::new(stream, EOF, []));
                mux.streams.lock().expect("tunnel lock").remove(&stream);
            }
        }
    });
}
