//! The chat with the CEO. Before the first message it is the whole window beside the sidebar: the
//! CEO and a box for the brief. After it, the conversation: the builder's messages, the CEO's
//! replies in Markdown, its questions waiting for an answer, and its reports.

mod attach;
mod composer;

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Entity, FontWeight, ScrollHandle, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use x100_client::Change;
use x100_core::agent::AgentState;
use x100_core::event::Seq;
use x100_core::file::FileRef;
use x100_core::fold::{ChatItem, ProjectPart, ProjectState, Record};
use x100_core::id::{MessageId, ProjectId};
use x100_core::message::Route;
use x100_core::task::Outcome;

pub use composer::Composer;

use crate::markdown::Markdown;
use crate::session::{Session, SessionEvent};
use crate::theme;
use crate::widgets::{self, Orb};

/// Whether the builder has sent the project's first message, the brief.
pub fn briefed(p: &ProjectState) -> bool {
    p.chat().iter().any(|c| match c.item {
        ChatItem::Message(id) => p
            .message(id)
            .is_some_and(|m| m.message.route == Route::BuilderToCeo),
        ChatItem::Report { .. } => false,
    })
}

/// A question from the CEO and where it stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Question {
    No,
    Open(MessageId),
    Answered,
}

/// One line of the chat, read from the state.
enum Line {
    Mine {
        body: String,
        files: Vec<FileRef>,
    },
    Ceo {
        seq: Seq,
        body: String,
        files: Vec<FileRef>,
        question: Question,
    },
    Daemon {
        body: String,
    },
    Report {
        seq: Seq,
        title: String,
        failed: bool,
        summary: String,
    },
}

pub struct ChatView {
    session: Entity<Session>,
    project: ProjectId,
    composer: Entity<Composer>,
    /// The CEO's replies and reports, parsed once.
    parsed: HashMap<Seq, Markdown>,
    scroll: ScrollHandle,
    lines: usize,
    _subscription: Subscription,
}

impl ChatView {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| Composer::new(session.clone(), project, cx));
        composer.update(cx, |c, cx| c.focus(window, cx));
        let subscription = cx.subscribe(&session, |this, _, event, cx| match event {
            SessionEvent::Changed(change) => this.changed(change, cx),
            SessionEvent::Preview => cx.notify(),
            SessionEvent::Selected => {}
        });
        Self {
            session,
            project,
            composer,
            parsed: HashMap::new(),
            scroll: ScrollHandle::new(),
            lines: 0,
            _subscription: subscription,
        }
    }

    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    fn changed(&mut self, change: &Change, cx: &mut Context<Self>) {
        match change {
            Change::Project { project, changes } if *project == self.project => {
                let ceo = self
                    .session
                    .read(cx)
                    .project(self.project, |p, _| p.project().map(|pr| pr.ceo))
                    .flatten();
                // The chat, and the CEO starting or stopping work, which the typing dots show.
                let ceo_changed =
                    ceo.is_some_and(|ceo| changes.records.contains(&Record::Agent(ceo)));
                if changes.parts.contains(&ProjectPart::Chat) || ceo_changed {
                    cx.notify();
                }
            }
            Change::Reset => cx.notify(),
            _ => {}
        }
    }

    fn read_lines(&self, cx: &Context<Self>) -> Option<(String, bool, bool, Vec<Line>)> {
        self.session.read(cx).project(self.project, |p, _| {
            let files = |ids: &[x100_core::id::FileId]| -> Vec<FileRef> {
                ids.iter()
                    .filter_map(|id| p.file(*id).map(|f| f.file.clone()))
                    .collect()
            };
            let lines = p
                .chat()
                .iter()
                .filter_map(|entry| match &entry.item {
                    ChatItem::Message(id) => {
                        let m = p.message(*id)?;
                        Some(match m.message.route {
                            Route::BuilderToCeo => Line::Mine {
                                body: m.message.body.clone(),
                                files: files(&m.message.attachments),
                            },
                            Route::DaemonToBuilder => Line::Daemon {
                                body: m.message.body.clone(),
                            },
                            _ => Line::Ceo {
                                seq: entry.seq,
                                body: m.message.body.clone(),
                                files: files(&m.message.attachments),
                                question: match (m.awaited, m.reply) {
                                    (false, _) => Question::No,
                                    (true, None) => Question::Open(m.message.id),
                                    (true, Some(_)) => Question::Answered,
                                },
                            },
                        })
                    }
                    ChatItem::Report { task, report } => Some(Line::Report {
                        seq: entry.seq,
                        title: p
                            .task(*task)
                            .map_or_else(|| "a task".into(), |t| t.task.title.clone()),
                        failed: !matches!(report.outcome, Outcome::Completed),
                        summary: report.summary.clone(),
                    }),
                })
                .collect();
            let name = p.project().map(|pr| pr.name.clone()).unwrap_or_default();
            let working = p
                .project()
                .and_then(|pr| p.agent(pr.ceo))
                .is_some_and(|a| a.state == AgentState::Working);
            (name, briefed(p), working, lines)
        })
    }

    fn markdown(&mut self, seq: Seq, body: &str) -> AnyElement {
        let id: SharedString = format!("md-{}", seq.0).into();
        self.parsed
            .entry(seq)
            .or_insert_with(|| Markdown::parse(body))
            .render(&id)
    }

    fn files(&self, files: &[FileRef], cx: &mut Context<Self>) -> Option<AnyElement> {
        if files.is_empty() {
            return None;
        }
        let project = self.project;
        let previews: Vec<_> = files
            .iter()
            .map(|f| {
                let image = self
                    .session
                    .update(cx, |s, cx| s.image(project, f.id, &f.media_type, cx));
                attach::preview(f, image)
            })
            .collect();
        Some(
            div()
                .mt(px(6.))
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(previews)
                .into_any_element(),
        )
    }

    fn line(&mut self, i: usize, line: Line, cx: &mut Context<Self>) -> AnyElement {
        let bubble = || {
            div()
                .flex_none()
                .max_w(gpui::relative(0.92))
                .px(px(11.))
                .py(px(8.))
                .rounded(px(14.))
                .text_size(px(theme::TEXT))
                .line_height(px(18.))
        };
        match line {
            Line::Mine { body, files } => {
                let files = self.files(&files, cx);
                bubble()
                    .self_end()
                    .rounded_br(px(4.))
                    .bg(theme::mine())
                    .border_1()
                    .border_color(gpui::rgba(0xffffff1a))
                    .text_color(theme::text())
                    .child(body)
                    .children(files)
                    .into_any_element()
            }
            Line::Ceo {
                seq,
                body,
                files,
                question,
            } => {
                let text = self.markdown(seq, &body);
                let files = self.files(&files, cx);
                let composer = self.composer.clone();
                bubble()
                    .self_start()
                    .rounded_bl(px(4.))
                    .bg(theme::theirs())
                    .text_color(theme::text())
                    .when(matches!(question, Question::Open(_)), |d| {
                        d.border_1().border_color(theme::ask())
                    })
                    .child(text)
                    .children(files)
                    .when_some(
                        match question {
                            Question::Open(id) => Some(id),
                            _ => None,
                        },
                        |d, id| {
                            d.child(
                                div()
                                    .mt(px(8.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(10.))
                                    .text_size(px(theme::TINY))
                                    .text_color(theme::ask())
                                    .child("Waiting for your answer")
                                    .child(
                                        widgets::button(("answer", i), "Answer")
                                            .debug_selector(|| "answer".into())
                                            .on_click(move |_, window, cx| {
                                                composer.update(cx, |c, cx| {
                                                    c.answer(id, cx);
                                                    c.focus(window, cx);
                                                })
                                            }),
                                    ),
                            )
                        },
                    )
                    .when(question == Question::Answered, |d| {
                        d.child(
                            div()
                                .mt(px(6.))
                                .text_size(px(theme::TINY))
                                .text_color(theme::dim())
                                .child("Answered"),
                        )
                    })
                    .into_any_element()
            }
            Line::Daemon { body } => bubble()
                .self_start()
                .border_1()
                .border_color(theme::line())
                .text_color(theme::muted())
                .child(body)
                .into_any_element(),
            Line::Report {
                seq,
                title,
                failed,
                summary,
            } => {
                let text = self.markdown(seq, &summary);
                bubble()
                    .self_start()
                    .rounded_bl(px(4.))
                    .bg(theme::theirs())
                    .text_color(theme::text())
                    .child(
                        div()
                            .mb(px(4.))
                            .text_size(px(theme::TINY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if failed {
                                theme::error()
                            } else {
                                theme::live()
                            })
                            .child(format!(
                                "{} · {title}",
                                if failed { "Failed" } else { "Done" }
                            )),
                    )
                    .child(text)
                    .into_any_element()
            }
        }
    }
}

fn typing() -> AnyElement {
    div()
        .self_start()
        .flex()
        .gap(px(4.))
        .px(px(12.))
        .py(px(11.))
        .rounded(px(14.))
        .rounded_bl(px(4.))
        .bg(theme::theirs())
        .children((0..3).map(|_| widgets::dot(5., theme::muted())))
        .into_any_element()
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((name, briefed, working, lines)) = self.read_lines(cx) else {
            return div().into_any_element();
        };
        if !briefed {
            // First run: the CEO, outlined, over the box for the brief.
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(px(20.))
                .child(widgets::orb(104., Orb::Outline).mb(px(26.)))
                .child(div().w(px(600.)).max_w_full().child(self.composer.clone()))
                .into_any_element();
        }
        if lines.len() != self.lines {
            self.lines = lines.len();
            self.scroll.scroll_to_bottom();
        }
        let elements: Vec<_> = lines
            .into_iter()
            .enumerate()
            .map(|(i, l)| self.line(i, l, cx))
            .collect();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px(px(16.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(theme::line())
                    .text_size(px(theme::TEXT))
                    .text_color(theme::muted())
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child("CEO"),
                            )
                            .child(format!("· {name}")),
                    ),
            )
            .child(
                div()
                    .id("messages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(14.))
                    // Few messages sit at the bottom, by the box, as in any chat.
                    .child(div().flex_1())
                    .children(elements)
                    .when(working, |d| d.child(typing())),
            )
            .child(
                div()
                    .flex_none()
                    .p(px(14.))
                    .border_t_1()
                    .border_color(theme::line())
                    .child(self.composer.clone()),
            )
            .into_any_element()
    }
}
