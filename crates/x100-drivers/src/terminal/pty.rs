//! Running a program in a pseudo-terminal: its bytes go to the supervisor and through an emulator
//! that answers the program's queries at once, whether or not anyone is watching.

use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use x100_core::terminal::TermPos;

use super::{Emulator, PROBE_ANSWER, ReportScanner, Reported, Size};
use crate::harness::CommandLine;
use crate::protocol::Key;
use crate::wire::Incoming;

type Writer = Arc<tokio::sync::Mutex<pty_process::OwnedWritePty>>;

/// A program's terminal, held by the supervisor.
pub(crate) struct Terminal {
    keys: mpsc::UnboundedSender<Vec<Key>>,
    control: mpsc::UnboundedSender<Control>,
    screen: Screen,
}

/// Requests the reader handles between reads, so they fall at an exact place in the recording.
enum Control {
    Resize(Size),
    Screen(oneshot::Sender<(TermPos, String)>),
}

/// The live screen, for a harness that reads what its program shows.
#[derive(Clone)]
pub(crate) struct Screen(Arc<Mutex<Emulator>>);

impl Screen {
    #[cfg(test)]
    pub fn blank(size: Size) -> Self {
        Self(Arc::new(Mutex::new(Emulator::new(size))))
    }

    pub fn text(&self) -> String {
        self.0.lock().expect("screen").text()
    }
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Terminal")
    }
}

impl Terminal {
    pub fn screen(&self) -> Screen {
        self.screen.clone()
    }

    pub fn type_keys(&self, keys: Vec<Key>) {
        let _ = self.keys.send(keys);
    }

    pub fn resize(&self, size: Size) {
        let _ = self.control.send(Control::Resize(size));
    }

    /// Asks for the screen as it is now. The answer arrives as [`Incoming::Screen`], after the
    /// output it reflects.
    pub fn screen_now(&self, reply: oneshot::Sender<(TermPos, String)>) {
        let _ = self.control.send(Control::Screen(reply));
    }
}

/// Starts `line` in a new pseudo-terminal of `size`, as the leader of a new session, so its
/// process group is its own. Its output, resizes and state reports arrive on `incoming`, the
/// starting size first, and [`Incoming::TerminalClosed`] once nothing holds the terminal open.
pub(crate) fn spawn(
    line: &CommandLine,
    env: &[(String, String)],
    dir: &Path,
    size: Size,
    incoming: mpsc::UnboundedSender<Incoming>,
) -> std::io::Result<(Child, Terminal)> {
    let (pty, pts) = pty_process::open().map_err(std::io::Error::other)?;
    pty.resize(pty_size(size)).map_err(std::io::Error::other)?;
    let child = pty_process::Command::new(&line.program)
        .args(&line.args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(dir)
        .kill_on_drop(true)
        .spawn(pts)
        .map_err(std::io::Error::other)?;
    let (read, write) = pty.into_split();
    let writer: Writer = Arc::new(tokio::sync::Mutex::new(write));
    let screen = Screen(Arc::new(Mutex::new(Emulator::new(size))));
    let (keys_tx, keys_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    tokio::spawn(type_keys(Arc::clone(&writer), keys_rx));
    let _ = incoming.send(Incoming::Resized(size));
    tokio::spawn(read_output(
        read,
        writer,
        screen.clone(),
        control_rx,
        incoming,
    ));
    Ok((
        child,
        Terminal {
            keys: keys_tx,
            control: control_tx,
            screen,
        },
    ))
}

fn pty_size(size: Size) -> pty_process::Size {
    pty_process::Size::new(size.rows, size.cols)
}

async fn type_keys(writer: Writer, mut keys: mpsc::UnboundedReceiver<Vec<Key>>) {
    while let Some(batch) = keys.recv().await {
        for key in batch {
            match key {
                Key::Bytes(bytes) => {
                    let mut w = writer.lock().await;
                    if w.write_all(&bytes).await.is_err() || w.flush().await.is_err() {
                        return;
                    }
                }
                Key::Pause(wait) => tokio::time::sleep(wait).await,
            }
        }
    }
}

async fn read_output(
    mut read: pty_process::OwnedReadPty,
    writer: Writer,
    screen: Screen,
    mut control: mpsc::UnboundedReceiver<Control>,
    incoming: mpsc::UnboundedSender<Incoming>,
) {
    let mut scanner = ReportScanner::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        tokio::select! {
            read = read.read(&mut buf) => {
                // EIO is how Linux says the last process holding the terminal has gone.
                let n = match read {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let bytes = buf[..n].to_vec();
                // Answered in the order asked: a program sends a query it may not get an answer
                // to, then one every terminal answers, and reads the first as unanswered if the
                // second's answer comes before it.
                let mut answer = Vec::new();
                let mut reports = Vec::new();
                let mut fed = 0;
                {
                    let mut emulator = screen.0.lock().expect("screen");
                    for (end, report) in scanner.scan(&bytes) {
                        answer.extend(emulator.feed(&bytes[fed..end]));
                        fed = end;
                        match report {
                            Reported::Probe => answer.extend_from_slice(PROBE_ANSWER),
                            report => reports.push(report),
                        }
                    }
                    answer.extend(emulator.feed(&bytes[fed..]));
                }
                if !answer.is_empty() {
                    let mut w = writer.lock().await;
                    let _ = w.write_all(&answer).await;
                    let _ = w.flush().await;
                }
                let _ = incoming.send(Incoming::Output(bytes));
                for report in reports {
                    let _ = incoming.send(Incoming::Reported(report));
                }
            }
            Some(request) = control.recv() => match request {
                // Resized here, between reads, so the recording has the new size exactly where
                // the program's output starts to follow it.
                Control::Resize(size) => {
                    let _ = writer.lock().await.resize(pty_size(size));
                    screen.0.lock().expect("screen").resize(size);
                    let _ = incoming.send(Incoming::Resized(size));
                }
                Control::Screen(reply) => {
                    let text = screen.text();
                    let _ = incoming.send(Incoming::Screen(text, reply));
                }
            },
        }
    }
    let _ = incoming.send(Incoming::TerminalClosed);
}
