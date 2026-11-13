//! What an agent is working in: its drivers' live output, one continuous view across a change of
//! driver.
//!
//! The agent's transcript is the backbone: it is followed from the start, and each driver's
//! `driver_changed` entry says when one driver gave way to the next. Consecutive headless drivers
//! share one log with the change marked in it. A terminal driver gets a screen of its own, replayed
//! from its first chunk at the sizes the daemon recorded and then followed live; its transcript is
//! not shown, since the screen shows that work. A change says which driver took over, why, and why
//! the last one ended, so it waits until the client's state holds the new driver.

use std::collections::{HashMap, HashSet, VecDeque};

use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use x100_client::output_log::{self, Change, LogWriter};
use x100_client::{Client, ClientError};
use x100_core::driver::{Purpose, StartReason, View};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{AgentId, DriverId, ProjectId};
use x100_core::terminal::{Chunk, ChunkKind};
use x100_core::transcript::{Entry, Item};
use x100_proto::ErrorCode;

use crate::ansi::AnsiLog;
use crate::msg::Msg;

/// Until a terminal's first chunk gives its size: the size the daemon starts terminals at.
const DEFAULT_SIZE: (u16, u16) = (40, 100);

pub enum Segment {
    Log(AnsiLog),
    Screen(Box<Screen>),
}

pub struct Screen {
    pub driver: DriverId,
    pub marker: String,
    pub parser: vt100::Parser,
    state: ScreenState,
}

enum ScreenState {
    /// Waiting for the daemon to take the subscription; the driver's entries are kept meanwhile,
    /// in case it cannot.
    Opening(Vec<Item>),
    Following,
    /// The daemon cannot show the terminal, so the driver's transcript is shown instead.
    Transcript(AnsiLog),
}

impl Screen {
    /// The rows of the screen as text.
    pub fn rows(&self) -> Vec<String> {
        match &self.state {
            ScreenState::Transcript(log) => log.text().lines().map(str::to_owned).collect(),
            _ => self
                .parser
                .screen()
                .rows(0, self.parser.screen().size().1)
                .collect(),
        }
    }
}

enum Read {
    Pending,
    NotTheAgents,
    Change(Change, View),
}

pub struct Output {
    pub agent: AgentId,
    project: ProjectId,
    token: u64,
    client: Client,
    tx: UnboundedSender<Msg>,
    handle: tokio::runtime::Handle,
    pub segments: Vec<Segment>,
    queue: VecDeque<Entry>,
    packs: HashMap<DriverId, usize>,
    hidden: HashSet<DriverId>,
    writer: LogWriter,
    tasks: Vec<JoinHandle<()>>,
    /// The cells last asked of live terminals, so the same size is not sent twice.
    sized: HashMap<DriverId, (u16, u16)>,
}

impl Drop for Output {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Output {
    pub fn open(
        client: Client,
        project: ProjectId,
        agent: AgentId,
        token: u64,
        tx: UnboundedSender<Msg>,
        handle: tokio::runtime::Handle,
    ) -> Output {
        let follow = {
            let (client, tx) = (client.clone(), tx.clone());
            handle.spawn(async move {
                let Ok(mut stream) = client.follow_transcript(project, agent, None).await else {
                    return;
                };
                while let Some((_, entry)) = stream.next().await {
                    if tx.send(Msg::Entry { token, entry }).is_err() {
                        return;
                    }
                }
            })
        };
        Output {
            agent,
            project,
            token,
            client,
            tx,
            handle,
            segments: Vec::new(),
            queue: VecDeque::new(),
            packs: HashMap::new(),
            hidden: HashSet::new(),
            writer: LogWriter::default(),
            tasks: vec![follow],
            sized: HashMap::new(),
        }
    }

    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn entry(&mut self, entry: Entry, p: &ProjectState, d: &DaemonState) {
        self.queue.push_back(entry);
        self.pump(p, d);
    }

    /// Shows queued entries in order, stopping at a change of driver the state cannot describe
    /// yet. The next change of state tries again.
    pub fn pump(&mut self, p: &ProjectState, d: &DaemonState) {
        while let Some(entry) = self.queue.front() {
            let driver = entry.driver;
            match &entry.item {
                Item::StartPack { text } => {
                    self.packs.insert(driver, text.len());
                }
                Item::DriverChanged { from } => match self.read_change(p, d, driver, *from) {
                    Read::Pending => return,
                    Read::NotTheAgents => {
                        self.packs.remove(&driver);
                        self.hidden.insert(driver);
                    }
                    Read::Change(change, view) => self.begin(driver, change, view),
                },
                _ if self.hidden.contains(&driver) => {}
                item => {
                    let item = item.clone();
                    self.show(driver, item);
                }
            }
            self.queue.pop_front();
        }
    }

    fn read_change(
        &self,
        p: &ProjectState,
        d: &DaemonState,
        driver: DriverId,
        from: Option<DriverId>,
    ) -> Read {
        let Some(record) = p.driver(driver) else {
            return Read::Pending;
        };
        if record.driver.purpose != Purpose::Work {
            return Read::NotTheAgents;
        }
        let number = |id: DriverId| {
            p.agent(self.agent)?
                .drivers
                .iter()
                .filter(|i| {
                    p.driver(**i)
                        .is_some_and(|r| r.driver.purpose == Purpose::Work)
                })
                .position(|i| *i == id)
                .map(|i| i + 1)
        };
        let previous = from.and_then(|old| {
            let state = p.driver(old).map(|r| &r.state);
            let why = match (state.and_then(output_log::ended), &record.driver.why) {
                (Some(ended), _) => ended,
                (None, StartReason::Handoff(h)) => output_log::handoff_cause(&h.cause),
                (None, _) => "it had not stopped yet".into(),
            };
            Some((number(old)?, why))
        });
        Read::Change(
            Change {
                number: number(driver).unwrap_or(1),
                label: output_log::driver_label(d, &record.driver),
                started_for: output_log::started_for(p, &record.driver.why),
                previous,
            },
            record.driver.view,
        )
    }

    fn begin(&mut self, driver: DriverId, change: Change, view: View) {
        let pack = self.packs.remove(&driver).map(|n| {
            output_log::notice(&format!(
                "  started from a start pack of {}",
                x100_client::words::size(n as u64)
            ))
        });
        match view {
            View::Headless => {
                let mut text = change.ansi();
                text.extend(pack);
                self.log().feed(&text);
            }
            View::Terminal => {
                let (rows, cols) = DEFAULT_SIZE;
                self.segments.push(Segment::Screen(Box::new(Screen {
                    driver,
                    marker: change.text(),
                    parser: vt100::Parser::new(rows, cols, 0),
                    state: ScreenState::Opening(Vec::new()),
                })));
                self.follow_terminal(driver);
            }
        }
    }

    fn follow_terminal(&mut self, driver: DriverId) {
        let (client, tx, project, token) = (
            self.client.clone(),
            self.tx.clone(),
            self.project,
            self.token,
        );
        self.tasks.push(self.handle.spawn(async move {
            let mut stream = match client.follow_terminal(project, driver, None).await {
                Ok(s) => s,
                Err(e) => {
                    let no_terminal =
                        matches!(&e, ClientError::Rpc(r) if r.code == ErrorCode::NotFound);
                    let _ = tx.send(Msg::TerminalRefused {
                        token,
                        driver,
                        no_terminal,
                        error: e.to_string(),
                    });
                    return;
                }
            };
            let _ = tx.send(Msg::TerminalOpened { token, driver });
            while let Some((_, chunk)) = stream.next().await {
                if tx
                    .send(Msg::Chunk {
                        token,
                        driver,
                        chunk,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    /// The log to write into: the newest segment if it is one, else a new one.
    fn log(&mut self) -> &mut AnsiLog {
        if !matches!(self.segments.last(), Some(Segment::Log(_))) {
            self.segments.push(Segment::Log(AnsiLog::default()));
        }
        match self.segments.last_mut() {
            Some(Segment::Log(log)) => log,
            _ => unreachable!("a log segment was just pushed"),
        }
    }

    fn show(&mut self, driver: DriverId, item: Item) {
        if let Some(Segment::Screen(s)) = self.segments.last_mut()
            && s.driver == driver
        {
            match &mut s.state {
                ScreenState::Opening(held) => held.push(item),
                ScreenState::Following => {}
                ScreenState::Transcript(log) => log.feed(&self.writer.item(&item)),
            }
            return;
        }
        let text = self.writer.item(&item);
        self.log().feed(&text);
    }

    fn screen_mut(&mut self, driver: DriverId) -> Option<&mut Screen> {
        self.segments.iter_mut().find_map(|s| match s {
            Segment::Screen(s) if s.driver == driver => Some(&mut **s),
            _ => None,
        })
    }

    pub fn terminal_opened(&mut self, driver: DriverId) {
        if let Some(s) = self.screen_mut(driver) {
            s.state = ScreenState::Following;
        }
    }

    pub fn chunk(&mut self, driver: DriverId, chunk: Chunk) {
        let Some(s) = self.screen_mut(driver) else {
            return;
        };
        match chunk.what {
            ChunkKind::Resize { cols, rows } => s.parser.set_size(rows, cols),
            ChunkKind::Output { bytes } => s.parser.process(&bytes.0),
        }
    }

    /// The daemon will not show this driver's terminal: say so in a plain line, and show its
    /// transcript in its place.
    pub fn terminal_refused(&mut self, driver: DriverId, no_terminal: bool, error: &str) {
        let line = if no_terminal {
            "This driver has no terminal to show. Its transcript follows.".to_owned()
        } else {
            format!("The terminal could not be followed ({error}). The transcript follows.")
        };
        let Some(s) = self.segments.iter_mut().find_map(|s| match s {
            Segment::Screen(s) if s.driver == driver => Some(s),
            _ => None,
        }) else {
            return;
        };
        let held = match std::mem::replace(&mut s.state, ScreenState::Following) {
            ScreenState::Opening(held) => held,
            _ => Vec::new(),
        };
        let mut log = AnsiLog::default();
        log.feed(&output_log::notice(&line));
        for item in &held {
            log.feed(&self.writer.item(item));
        }
        s.state = ScreenState::Transcript(log);
    }

    /// Asks a live terminal driver's program to draw at the pane's size, once per size.
    pub fn resize(&mut self, p: &ProjectState, cols: u16, rows: u16) {
        let (min, max) = (
            x100_proto::ResizeTerminal::MIN,
            x100_proto::ResizeTerminal::MAX,
        );
        let (cols, rows) = (cols.clamp(min.0, max.0), rows.clamp(min.1, max.1));
        let live: Vec<DriverId> = self
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Screen(s) if p.driver(s.driver).is_some_and(|d| d.state.is_live()) => {
                    Some(s.driver)
                }
                _ => None,
            })
            .collect();
        for driver in live {
            if self.sized.get(&driver) == Some(&(cols, rows)) {
                continue;
            }
            self.sized.insert(driver, (cols, rows));
            let (client, project) = (self.client.clone(), self.project);
            // A refusal, the driver having ended meanwhile, leaves the screen as it last drew.
            self.handle.spawn(async move {
                let _ = client.resize_terminal(project, driver, cols, rows).await;
            });
        }
    }

    /// All the text shown, for a test: each segment's marker and rows.
    pub fn text(&self) -> String {
        self.segments
            .iter()
            .map(|s| match s {
                Segment::Log(l) => l.text(),
                Segment::Screen(s) => format!("{}\n{}", s.marker, s.rows().join("\n")),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `claude` terminal recorded at 120 by 40, replayed in the order the program wrote it.
    #[test]
    fn a_recorded_terminal_becomes_the_screen_rows() {
        let raw = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../x100-app/tests/data/claude.raw"
        ))
        .expect("the recording");
        let mut screen = Screen {
            driver: DriverId::from_u128(1),
            marker: String::new(),
            parser: vt100::Parser::new(40, 120, 0),
            state: ScreenState::Following,
        };
        screen.parser.process(&raw);
        let rows = screen.rows();
        assert_eq!(rows.len(), 40);
        // The program ends by leaving its full-screen mode and saying how to resume.
        assert!(rows.iter().any(|r| r.contains("Resume this session with:")));
        assert!(rows.iter().any(|r| r.contains("claude --resume ")));
    }
}
