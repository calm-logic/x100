//! The Log tab: every action by every agent, newest first, one line each.
//!
//! It reads the project's log from the daemon a page at a time with `events.read`, newest first,
//! and follows it by reading what came after the newest event it holds whenever the client's state
//! says there is more. Lines are made once, as the events arrive, and only the rows in view are
//! laid out, so a log of tens of thousands of events costs what a screenful does.

use x100_client::wording;

use std::collections::{HashMap, VecDeque};

use gpui::{
    Context, Entity, ScrollStrategy, Subscription, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};
use x100_client::Change;
use x100_core::appearance::Appearance;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::fold::ProjectPart;
use x100_core::id::{AgentId, FileId, ProjectId};
use x100_core::party::Actor;

pub use wording::{Line, Names, kind, line, request};

use crate::face;
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, words};

pub const ROW_HEIGHT: f32 = 36.;
/// Events in the page read when the tab opens and when the list is scrolled near its end.
const PAGE: u32 = 200;
/// Events read at a time when catching up with new ones.
const CATCH_UP: u32 = 500;
/// How close to the end of what is loaded the list may scroll before it reads an older page.
const LOOK_AHEAD: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub seq: Seq,
    pub at: String,
    /// The agent that acted, whose face the row shows. The builder and x100 have none.
    pub by: Option<AgentId>,
    pub who: String,
    pub text: String,
}

pub struct LogView {
    session: Entity<Session>,
    project: ProjectId,
    /// Newest first.
    lines: VecDeque<LogLine>,
    newest: Option<Seq>,
    oldest: Option<Seq>,
    /// There are events older than the oldest loaded.
    older: bool,
    loading: bool,
    /// The list was scrolled near its end while a read was going.
    wants_older: bool,
    /// Names of files from the events that added them, for those the session never saw.
    files: HashMap<FileId, String>,
    scroll: UniformListScrollHandle,
    /// How many rows the last draw laid out, to show that the rest are not.
    drawn: usize,
    error: Option<Outcome>,
    _subscription: Subscription,
}

impl LogView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            if let SessionEvent::Changed(change) = event {
                this.changed(change, cx);
            }
        });
        let mut view = Self {
            session,
            project,
            lines: VecDeque::new(),
            newest: None,
            oldest: None,
            older: false,
            loading: false,
            wants_older: false,
            files: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            drawn: 0,
            error: None,
            _subscription: subscription,
        };
        view.sync(cx);
        view
    }

    /// The lines loaded, newest first.
    pub fn lines(&self) -> &VecDeque<LogLine> {
        &self.lines
    }

    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// Whether every event up to the client's newest is loaded and nothing is being read.
    pub fn caught_up(&self, cx: &gpui::App) -> bool {
        let seq = self
            .session
            .read(cx)
            .project(self.project, |p, _| p.seq())
            .flatten();
        !self.loading && self.newest == seq
    }

    fn looks(&self, cx: &gpui::App) -> HashMap<AgentId, Appearance> {
        self.session
            .read(cx)
            .project(self.project, |p, _| {
                p.agents().map(|a| (a.id, a.appearance)).collect()
            })
            .unwrap_or_default()
    }

    fn changed(&mut self, change: &Change, cx: &mut Context<Self>) {
        match change {
            Change::Project { project, changes }
                if *project == self.project && changes.parts.contains(&ProjectPart::Looks) =>
            {
                self.sync(cx);
                cx.notify();
            }
            Change::Project { project, changes }
                if *project == self.project && changes.parts.contains(&ProjectPart::Log) =>
            {
                self.sync(cx);
            }
            Change::Reset => {
                self.lines.clear();
                self.newest = None;
                self.oldest = None;
                self.sync(cx);
            }
            _ => {}
        }
    }

    /// Reads whatever is missing: the first page, what came after the newest line, or an older
    /// page, one read at a time.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let session = self.session.read(cx);
        let client = session.client().clone();
        let project = self.project;
        let held = session.project(project, |p, _| p.seq()).flatten();
        let (newest, oldest) = (self.newest, self.oldest);
        enum Read {
            First,
            After(Seq),
            Before(Seq),
        }
        let read = match (newest, oldest) {
            (None, _) if held.is_some() => Read::First,
            (Some(n), _) if held > Some(n) => Read::After(n),
            (_, Some(o)) if self.older && self.wants_older => Read::Before(o),
            _ => return,
        };
        self.loading = true;
        let request = async move {
            match read {
                Read::First => (
                    false,
                    client.read_events_before(project, None, PAGE).await,
                    PAGE,
                ),
                Read::After(n) => (
                    true,
                    client.read_events_after(project, Some(n), CATCH_UP).await,
                    CATCH_UP,
                ),
                Read::Before(o) => (
                    false,
                    client.read_events_before(project, Some(o), PAGE).await,
                    PAGE,
                ),
            }
        };
        cx.spawn(async move |this, cx| {
            let (forward, result, limit) = request.await;
            let _ = this.update(cx, |view, cx| {
                view.loading = false;
                match result {
                    Ok(events) => {
                        view.error = None;
                        // An empty read cannot be asked again to any more effect.
                        if view.ingest(events, forward, limit, cx) {
                            view.sync(cx);
                        }
                    }
                    Err(e) => view.error = Some(Outcome::of(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Takes a page in, and says whether it held anything. `forward` pages are oldest first and
    /// newer than everything held; the others are newest first and older.
    fn ingest(
        &mut self,
        mut events: Vec<Envelope<ProjectEvent>>,
        forward: bool,
        limit: u32,
        cx: &mut Context<Self>,
    ) -> bool {
        if !forward {
            events.reverse();
            self.older =
                events.len() as u32 == limit && events.first().is_some_and(|e| e.seq > Seq(1));
        }
        // Oldest first from here, so an added file is known by the time its removal is read.
        for e in &events {
            if let ProjectEvent::FileAdded(f) = &e.event {
                self.files.insert(f.id, f.name.clone());
            }
        }
        let session = self.session.read(cx);
        let mut files = session.file_names().clone();
        files.extend(self.files.iter().map(|(k, v)| (*k, v.clone())));
        let made: Vec<LogLine> = session
            .project(self.project, |p, daemon| {
                let names = Names {
                    project: p,
                    daemon,
                    files: &files,
                };
                events
                    .iter()
                    .map(|e| {
                        let l = line(e, &names);
                        LogLine {
                            seq: e.seq,
                            at: words::moment(e.at),
                            by: match e.by {
                                Actor::Agent(a) => Some(a),
                                Actor::Builder | Actor::Daemon => None,
                            },
                            who: l.who,
                            text: l.text,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (Some(first), Some(last)) = (made.first(), made.last()) else {
            return false;
        };
        if forward {
            let added = made.len();
            self.newest = Some(last.seq);
            self.oldest.get_or_insert(first.seq);
            for l in made {
                self.lines.push_front(l);
            }
            // A reader scrolled down keeps their place while new lines arrive above.
            let top = self.scroll.0.borrow().base_handle.logical_scroll_top().0;
            if top > 0 {
                self.scroll
                    .scroll_to_item_strict(top + added, ScrollStrategy::Top);
            }
        } else {
            self.oldest = Some(first.seq);
            self.newest.get_or_insert(last.seq);
            self.wants_older = false;
            for l in made.into_iter().rev() {
                self.lines.push_back(l);
            }
        }
        true
    }
}

fn row(l: &LogLine, look: Option<Appearance>) -> gpui::Div {
    div()
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .border_b_1()
        .border_color(theme::line())
        .text_size(px(theme::TEXT))
        .child(
            div()
                .flex_none()
                .w(px(104.))
                .font_family(theme::MONO)
                .text_size(px(theme::TINY))
                .text_color(theme::dim())
                .child(l.at.clone()),
        )
        .child(look.map_or_else(|| face::blank(18.), |l| face::face(18., l)))
        .child(
            div()
                .flex_none()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(l.who.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(theme::muted())
                .child(l.text.clone()),
        )
}

impl Render for LogView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.lines.len();
        let body = if count == 0 {
            div()
                .p(px(24.))
                .text_size(px(theme::TEXT))
                .text_color(theme::dim())
                .child("Nothing has happened yet. Every action by every agent is listed here.")
                .into_any_element()
        } else {
            uniform_list(
                "log",
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    this.drawn = range.len();
                    if range.end + LOOK_AHEAD >= this.lines.len() && this.older {
                        this.wants_older = true;
                        this.sync(cx);
                    }
                    // Looked up as drawn, so a changed look shows on rows made before it.
                    let looks = this.looks(cx);
                    range
                        .filter_map(|i| {
                            let l = this.lines.get(i)?;
                            Some(row(l, l.by.and_then(|a| looks.get(&a).copied())))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        div()
            .id("log")
            .size_full()
            .flex()
            .flex_col()
            .when_some(self.error.as_ref(), |d, e| {
                d.child(div().px(px(16.)).py(px(8.)).child(notice::line(e)))
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}
