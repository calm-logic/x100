//! The Specs tab: the BRD, PRD and ADD, each with its author, revision and status, as Markdown.
//! Read-only: the agents write them.

use gpui::{Context, Entity, FontWeight, SharedString, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::{ProjectPart, ProjectState};
use x100_core::id::ProjectId;
use x100_core::spec::{SpecKind, SpecStatus};

use crate::markdown::Markdown;
use crate::session::{Session, SessionEvent};
use crate::widgets::{self, Orb};
use crate::{theme, words};

const KINDS: [SpecKind; 3] = [SpecKind::Brd, SpecKind::Prd, SpecKind::Add];

/// What the tab shows of one spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecModel {
    pub kind: SpecKind,
    pub author: String,
    pub revision: u32,
    pub status: SpecStatus,
    pub body: String,
}

impl SpecModel {
    pub fn read(p: &ProjectState, kind: SpecKind) -> Option<SpecModel> {
        let s = &p.spec(kind)?.spec;
        Some(SpecModel {
            kind,
            author: p
                .agent(s.author)
                .map_or_else(|| "an agent".into(), |a| a.role.clone()),
            revision: s.revision,
            status: s.status,
            body: s.body.clone(),
        })
    }
}

pub struct SpecsView {
    session: Entity<Session>,
    project: ProjectId,
    open: SpecKind,
    model: Option<SpecModel>,
    /// The open spec's body, parsed once for each revision drawn.
    parsed: Option<(u32, SpecKind, Markdown)>,
    _subscription: Subscription,
}

impl SpecsView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project && changes.parts.contains(&ProjectPart::Specs)
                }
                SessionEvent::Changed(Change::Reset) => true,
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let mut view = Self {
            session,
            project,
            open: SpecKind::Brd,
            model: None,
            parsed: None,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// The spec shown, or `None` while the open one is not written.
    pub fn shown(&self) -> Option<&SpecModel> {
        self.model.as_ref()
    }

    pub fn open(&mut self, kind: SpecKind, cx: &mut Context<Self>) {
        self.open = kind;
        self.refresh(cx);
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let kind = self.open;
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| SpecModel::read(p, kind))
            .flatten();
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }
}

fn status(status: SpecStatus) -> impl IntoElement {
    let (label, colour) = match status {
        SpecStatus::Draft => ("Draft", theme::ask()),
        SpecStatus::Approved => ("Approved", theme::live()),
    };
    div()
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .border_1()
        .border_color(theme::line())
        .text_size(px(theme::TINY))
        .text_color(colour)
        .child(label)
}

impl Render for SpecsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open;
        let switch = widgets::segments(KINDS.map(|kind| {
            widgets::segment(
                words::spec_abbreviation(kind),
                words::spec_short(kind),
                kind == open,
            )
            .debug_selector(move || format!("spec-{}", words::spec_abbreviation(kind)))
            .on_click(cx.listener(move |this, _, _, cx| this.open(kind, cx)))
            .into_any_element()
        }));
        let doc = match self.model.clone() {
            None => div()
                .p(px(8.))
                .text_size(px(theme::TEXT))
                .text_color(theme::dim())
                .child(format!(
                    "The {} is not written yet. The CEO writes the business requirements first, then Product and Tech write theirs.",
                    words::spec_name(open).to_lowercase()
                ))
                .into_any_element(),
            Some(m) => {
                if self
                    .parsed
                    .as_ref()
                    .is_none_or(|(rev, kind, _)| (*rev, *kind) != (m.revision, m.kind))
                {
                    self.parsed = Some((m.revision, m.kind, Markdown::parse(&m.body)));
                }
                let id: SharedString = format!("spec-{}", words::spec_abbreviation(m.kind)).into();
                let body = self.parsed.as_ref().map(|(_, _, md)| md.render(&id));
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .p(px(20.))
                    .rounded(px(14.))
                    .border_1()
                    .border_color(theme::line())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .pb(px(14.))
                            .border_b_1()
                            .border_color(theme::line())
                            .child(widgets::orb(32., Orb::Agent))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(16.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(words::spec_name(m.kind)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(theme::SMALL))
                                            .text_color(theme::muted())
                                            .child(format!(
                                                "{} \u{b7} written by {} \u{b7} revision {}",
                                                words::spec_abbreviation(m.kind),
                                                m.author,
                                                m.revision
                                            )),
                                    ),
                            )
                            .child(status(m.status)),
                    )
                    .child(
                        div()
                            .text_size(px(theme::TEXT))
                            .text_color(theme::body())
                            .children(body),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("specs")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(16.))
            .child(div().flex().child(switch))
            .child(doc)
    }
}
