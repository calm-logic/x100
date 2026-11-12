//! What an agent that runs no agents of its own is working in: its drivers' live output, one
//! continuous view across a change of driver.
//!
//! The agent's transcript is the backbone: it is followed from the start, and each driver's
//! `driver_changed` entry says when one driver gave way to the next. A headless driver's entries
//! are written into a log as they come. A terminal driver gets a screen of its own, replayed from
//! its first chunk at the sizes the daemon recorded and then followed live, and its transcript
//! entries are not shown, since the screen shows that work. Consecutive headless drivers share
//! one log, with the change marked in it; a terminal driver's screen has the change marked above
//! it. Each change says which driver took over (harness, model, effort), why, and why the last
//! one ended, so it waits until the client's state holds the new driver.
//!
//! Output arriving redraws only the screen it lands in: each segment's terminal is a cached view
//! of its own. GPUI runs this view's render again too, as it does for every
//! ancestor of a changed view, but that rebuilds only the frame and reuses the cached children.

use std::collections::{HashMap, HashSet, VecDeque};

use gpui::{
    Context, Entity, ScrollHandle, SharedString, StyleRefinement, Subscription, Task, Window, div,
    prelude::*, px,
};
use x100_client::{Change, ClientError};
use x100_core::driver::{Purpose, StartReason, View};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId, ProjectId};
use x100_core::terminal::{Chunk, ChunkKind};
use x100_core::transcript::{Entry, Item};
use x100_proto::ErrorCode;

use super::log::{self, LogWriter};
use crate::session::{Session, SessionEvent};
use crate::term::{Sizing, TerminalView};
use crate::theme;

/// Until a terminal's first chunk gives its size.
const DEFAULT_SIZE: (usize, usize) = (120, 40);
/// An earlier segment keeps this much room; the newest takes the rest.
const EARLIER_HEIGHT: f32 = 220.;
const NEWEST_MIN_HEIGHT: f32 = 300.;

/// A run of output in one terminal widget.
struct Segment {
    view: Entity<TerminalView>,
    /// The change of driver above a terminal driver's screen.
    marker: Option<SharedString>,
    kind: Kind,
}

enum Kind {
    /// Headless drivers' transcripts, written as a log.
    Log,
    Screen {
        driver: DriverId,
        state: Screen,
        _follow: Task<()>,
    },
}

enum Screen {
    /// Waiting for the daemon to take the subscription; the driver's entries are kept meanwhile,
    /// in case it cannot.
    Opening(Vec<Item>),
    Following,
    /// The daemon cannot show the terminal, so the driver's transcript is shown instead.
    Transcript,
}

/// What the state says about a change of driver.
enum Read {
    /// It does not have the new driver's record yet.
    Pending,
    /// A lab driver, which runs a trial and does not act for the agent.
    NotTheAgents,
    Change(log::Change, View),
}

/// A segment as tests see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentShown {
    pub terminal_of: Option<DriverId>,
    pub marker: Option<String>,
    pub text: String,
    pub screen: Vec<String>,
}

pub struct AgentOutput {
    session: Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    segments: Vec<Segment>,
    /// Entries not yet shown, in order: the first waits until the state can describe it.
    queue: VecDeque<Entry>,
    /// Each driver's start pack, by size, shown once its driver has been marked.
    packs: HashMap<DriverId, usize>,
    /// Drivers whose entries are not shown: lab drivers.
    hidden: HashSet<DriverId>,
    writer: LogWriter,
    scroll: ScrollHandle,
    renders: usize,
    _transcript: Task<()>,
    _subscription: Subscription,
}

impl AgentOutput {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&session, move |this, _, event, cx| match event {
            SessionEvent::Changed(Change::Project { project, .. }) if *project == this.project => {
                this.state_changed(cx)
            }
            SessionEvent::Changed(Change::Reset) => this.state_changed(cx),
            _ => {}
        });
        let client = session.read(cx).client().clone();
        let transcript = cx.spawn(async move |this, cx| {
            let mut stream = match client.follow_transcript(project, agent, None).await {
                Ok(stream) => stream,
                Err(e) => {
                    let _ = this.update(cx, |o, cx| {
                        o.notice(&format!("The transcript could not be followed: {e}"), cx)
                    });
                    return;
                }
            };
            while let Some((_, entry)) = stream.next().await {
                if this.update(cx, |o, cx| o.entry(entry, cx)).is_err() {
                    return;
                }
            }
        });
        Self {
            session,
            project,
            agent,
            segments: Vec::new(),
            queue: VecDeque::new(),
            packs: HashMap::new(),
            hidden: HashSet::new(),
            writer: LogWriter::default(),
            scroll: ScrollHandle::new(),
            renders: 0,
            _transcript: transcript,
            _subscription: subscription,
        }
    }

    pub fn agent(&self) -> AgentId {
        self.agent
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn terminals(&self) -> Vec<Entity<TerminalView>> {
        self.segments.iter().map(|s| s.view.clone()).collect()
    }

    pub fn shown(&self, cx: &gpui::App) -> Vec<SegmentShown> {
        self.segments
            .iter()
            .map(|s| {
                let grid = s.view.read(cx).grid();
                SegmentShown {
                    terminal_of: match &s.kind {
                        Kind::Screen { driver, .. } => Some(*driver),
                        Kind::Log => None,
                    },
                    marker: s.marker.as_ref().map(|m| m.to_string()),
                    text: grid.all_text(),
                    screen: grid.screen_text(),
                }
            })
            .collect()
    }

    fn state_changed(&mut self, cx: &mut Context<Self>) {
        self.pump(cx);
    }

    fn entry(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.queue.push_back(entry);
        self.pump(cx);
    }

    /// Shows queued entries in order, stopping at a change of driver the state cannot describe
    /// yet. The next change of state tries again.
    fn pump(&mut self, cx: &mut Context<Self>) {
        while let Some(entry) = self.queue.front() {
            let driver = entry.driver;
            match &entry.item {
                Item::StartPack { text } => {
                    self.packs.insert(driver, text.len());
                }
                Item::DriverChanged { from } => match self.read_change(driver, *from, cx) {
                    Read::Pending => return,
                    Read::NotTheAgents => {
                        self.packs.remove(&driver);
                        self.hidden.insert(driver);
                    }
                    Read::Change(change, view) => self.begin(driver, change, view, cx),
                },
                _ if self.hidden.contains(&driver) => {}
                item => {
                    let item = item.clone();
                    self.show(driver, item, cx);
                }
            }
            self.queue.pop_front();
        }
    }

    fn state<R>(&self, cx: &Context<Self>, f: impl FnOnce(&ProjectState) -> R) -> Option<R> {
        self.session.read(cx).project(self.project, |p, _| f(p))
    }

    /// The change to `driver`, once the state has its record. The driver before it ended before
    /// it started, and the client applies the log in order, so that end is known by then.
    fn read_change(&self, driver: DriverId, from: Option<DriverId>, cx: &Context<Self>) -> Read {
        let agent = self.agent;
        self.session
            .read(cx)
            .project(self.project, |p, daemon| {
                let Some(record) = p.driver(driver) else {
                    return Read::Pending;
                };
                if record.driver.purpose != Purpose::Work {
                    return Read::NotTheAgents;
                }
                let number = |d: DriverId| {
                    p.agent(agent)?
                        .drivers
                        .iter()
                        .filter(|id| {
                            p.driver(**id)
                                .is_some_and(|r| r.driver.purpose == Purpose::Work)
                        })
                        .position(|id| *id == d)
                        .map(|i| i + 1)
                };
                let previous = from.and_then(|old| {
                    let state = p.driver(old).map(|r| &r.state);
                    let why = match (state.and_then(log::ended), &record.driver.why) {
                        (Some(ended), _) => ended,
                        (None, StartReason::Handoff(h)) => log::handoff_cause(&h.cause),
                        (None, _) => "it had not stopped yet".into(),
                    };
                    Some((number(old)?, why))
                });
                let change = log::Change {
                    number: number(driver).unwrap_or(1),
                    label: log::driver_label(daemon, &record.driver),
                    started_for: log::started_for(p, &record.driver.why),
                    previous,
                };
                Read::Change(change, record.driver.view)
            })
            .unwrap_or(Read::Pending)
    }

    fn begin(&mut self, driver: DriverId, change: log::Change, view: View, cx: &mut Context<Self>) {
        let pack = self
            .packs
            .remove(&driver)
            .map(|n| log::notice(&format!("  started from a start pack of {}", words(n))));
        match view {
            View::Headless => {
                let mut text = change.ansi();
                text.extend(pack);
                self.log_segment(cx)
                    .update(cx, |v, cx| v.feed(text.as_bytes(), cx));
            }
            View::Terminal => {
                let (cols, rows) = DEFAULT_SIZE;
                let view = cx.new(|cx| TerminalView::new(Sizing::Recorded, cols, rows, cx));
                let follow = self.follow(driver, cx);
                self.segments.push(Segment {
                    view,
                    marker: Some(change.text().into()),
                    kind: Kind::Screen {
                        driver,
                        state: Screen::Opening(Vec::new()),
                        _follow: follow,
                    },
                });
                self.scroll.scroll_to_bottom();
                cx.notify();
            }
        }
    }

    /// The log to write into: the newest segment if it is one, else a new one.
    fn log_segment(&mut self, cx: &mut Context<Self>) -> Entity<TerminalView> {
        if let Some(Segment {
            view,
            kind: Kind::Log,
            ..
        }) = self.segments.last()
        {
            return view.clone();
        }
        let view = log_view(cx);
        self.segments.push(Segment {
            view: view.clone(),
            marker: None,
            kind: Kind::Log,
        });
        self.scroll.scroll_to_bottom();
        cx.notify();
        view
    }

    /// Shows one transcript item of `driver`'s.
    fn show(&mut self, driver: DriverId, item: Item, cx: &mut Context<Self>) {
        if let Some(Segment {
            view,
            kind: Kind::Screen {
                driver: d, state, ..
            },
            ..
        }) = self.segments.last_mut()
            && *d == driver
        {
            match state {
                Screen::Opening(held) => held.push(item),
                Screen::Following => {}
                Screen::Transcript => {
                    let text = self.writer.item(&item);
                    view.update(cx, |v, cx| v.feed(text.as_bytes(), cx));
                }
            }
            return;
        }
        let text = self.writer.item(&item);
        self.log_segment(cx)
            .update(cx, |v, cx| v.feed(text.as_bytes(), cx));
    }

    fn follow(&self, driver: DriverId, cx: &mut Context<Self>) -> Task<()> {
        let client = self.session.read(cx).client().clone();
        let project = self.project;
        cx.spawn(async move |this, cx| {
            let mut stream = match client.follow_terminal(project, driver, None).await {
                Ok(stream) => stream,
                Err(e) => {
                    let _ = this.update(cx, |o, cx| o.refused(driver, e, cx));
                    return;
                }
            };
            if this.update(cx, |o, _| o.opened(driver)).is_err() {
                return;
            }
            while let Some((_, chunk)) = stream.next().await {
                if this.update(cx, |o, cx| o.chunk(driver, chunk, cx)).is_err() {
                    return;
                }
            }
        })
    }

    fn screen_mut(&mut self, driver: DriverId) -> Option<(&Entity<TerminalView>, &mut Screen)> {
        self.segments.iter_mut().find_map(|s| match &mut s.kind {
            Kind::Screen {
                driver: d, state, ..
            } if *d == driver => Some((&s.view, state)),
            _ => None,
        })
    }

    fn opened(&mut self, driver: DriverId) {
        if let Some((_, state)) = self.screen_mut(driver) {
            *state = Screen::Following;
        }
    }

    fn chunk(&mut self, driver: DriverId, chunk: Chunk, cx: &mut Context<Self>) {
        let Some((view, _)) = self.screen_mut(driver) else {
            return;
        };
        view.update(cx, |v, cx| match chunk.what {
            ChunkKind::Resize { cols, rows } => v.resize(cols.into(), rows.into(), cx),
            ChunkKind::Output { bytes } => v.feed(&bytes.0, cx),
        });
    }

    /// The daemon will not show this driver's terminal: say so in a plain line, and show its
    /// transcript in its place.
    fn refused(&mut self, driver: DriverId, error: ClientError, cx: &mut Context<Self>) {
        let line = match &error {
            ClientError::Rpc(e) if e.code == ErrorCode::NotFound => {
                "This driver has no terminal to show. Its transcript follows.".into()
            }
            e => format!("The terminal could not be followed ({e}). The transcript follows."),
        };
        let log = log_view(cx);
        let Some(segment) = self
            .segments
            .iter_mut()
            .find(|s| matches!(&s.kind, Kind::Screen { driver: d, .. } if *d == driver))
        else {
            return;
        };
        let Kind::Screen { state, .. } = &mut segment.kind else {
            return;
        };
        let held = match std::mem::replace(state, Screen::Transcript) {
            Screen::Opening(held) => held,
            Screen::Following | Screen::Transcript => Vec::new(),
        };
        let mut text = log::notice(&line);
        for item in &held {
            text.push_str(&self.writer.item(item));
        }
        log.update(cx, |v, cx| v.feed(text.as_bytes(), cx));
        segment.view = log;
        cx.notify();
    }

    /// A line of the view's own, when there is nothing else to show.
    fn notice(&mut self, text: &str, cx: &mut Context<Self>) {
        let line = log::notice(text);
        self.log_segment(cx)
            .update(cx, |v, cx| v.feed(line.as_bytes(), cx));
    }

    fn bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let agent = self.agent;
        let role = self
            .state(cx, |p| p.agent(agent).map(|a| a.role.clone()))
            .flatten()
            .unwrap_or_default();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme::line())
            .bg(gpui::rgb(0x161617))
            .font_family(theme::MONO)
            .text_size(px(theme::TINY))
            .text_color(theme::muted())
            .children((0..3).map(|_| crate::widgets::dot(9., gpui::rgb(0x3a3a3d))))
            .child(div().ml(px(6.)).child(format!("{role} · live output")))
    }
}

/// A terminal for a log the app writes. It has no cursor: nothing is typed there.
fn log_view<T>(cx: &mut Context<T>) -> Entity<TerminalView> {
    let (cols, rows) = DEFAULT_SIZE;
    cx.new(|cx| {
        let mut view = TerminalView::new(Sizing::Fit, cols, rows, cx);
        view.feed(b"\x1b[?25l", cx);
        view
    })
}

/// A start pack's size in words a builder reads: "4 KB".
fn words(bytes: usize) -> String {
    crate::words::size(bytes as u64)
}

impl Render for AgentOutput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let last = self.segments.len().saturating_sub(1);
        let segments = self.segments.iter().enumerate().map(|(i, s)| {
            let newest = i == last;
            div()
                .w_full()
                .flex()
                .flex_col()
                .when(newest, |d| d.flex_1().min_h(px(NEWEST_MIN_HEIGHT)))
                .when(!newest, |d| d.flex_none().h(px(EARLIER_HEIGHT)))
                .when_some(s.marker.clone(), |d, marker| {
                    d.child(
                        div()
                            .flex_none()
                            .px(px(10.))
                            .py(px(5.))
                            .border_t_1()
                            .border_color(theme::line())
                            .font_family(theme::MONO)
                            .text_size(px(theme::TINY))
                            .text_color(theme::ask())
                            .child(marker),
                    )
                })
                .child(
                    div().flex_1().min_h_0().child(
                        s.view
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
                )
        });
        div()
            .size_full()
            .min_h(px(NEWEST_MIN_HEIGHT))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(theme::line())
            .bg(gpui::rgb(crate::term::BACKGROUND))
            .overflow_hidden()
            .child(self.bar(cx))
            .child(
                div()
                    .id("agent-output")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(segments)
                    .when(self.segments.is_empty(), |d| {
                        d.child(
                            div()
                                .p(px(14.))
                                .font_family(theme::MONO)
                                .text_size(px(theme::SMALL))
                                .text_color(theme::dim())
                                .child("No driver has worked for this agent yet."),
                        )
                    }),
            )
    }
}
