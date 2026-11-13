//! The Tasks tab: every task as a flat list, with a way for the builder to raise one. Each row is
//! a cached view of its own, so one task changing redraws that row alone.

use std::collections::HashMap;

use gpui::{
    Context, Entity, FontWeight, Rgba, StyleRefinement, Subscription, Window, div, prelude::*, px,
};
use x100_client::Change;
use x100_core::fold::{ProjectPart, ProjectState, Record};
use x100_core::id::{ProjectId, TaskId};
use x100_core::party::Party;
use x100_core::task::TaskState;

use crate::glass;
use crate::input::{InputEvent, TextInput};
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::theme;
use crate::widgets;

pub const ROW_HEIGHT: f32 = 58.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NewIdea,
    InProgress,
    InReview,
    Promoting,
    Done,
}

impl Status {
    pub fn of(state: &TaskState) -> Status {
        match state {
            TaskState::NewIdea => Status::NewIdea,
            TaskState::InProgress(_) => Status::InProgress,
            TaskState::InReview(..) => Status::InReview,
            TaskState::Promoting(..) => Status::Promoting,
            TaskState::Done(..) => Status::Done,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::NewIdea => "New idea",
            Status::InProgress => "In progress",
            Status::InReview => "In review",
            Status::Promoting => "Promoting",
            Status::Done => "Done",
        }
    }

    fn colour(self) -> Rgba {
        match self {
            Status::NewIdea => theme::muted(),
            Status::InProgress => theme::ask(),
            Status::InReview => gpui::rgb(0xb79df5),
            Status::Promoting => theme::link(),
            Status::Done => theme::live(),
        }
    }
}

/// What a row shows, read from the folded state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskModel {
    pub id: TaskId,
    /// Its place in the order the tasks were opened, which never changes.
    pub number: usize,
    pub title: String,
    pub status: Status,
    /// An agent is at work on it.
    pub live: bool,
    pub part: String,
    pub raised_by: String,
}

impl TaskModel {
    /// Every task, in the order it was opened.
    pub fn all(p: &ProjectState) -> Vec<TaskModel> {
        let mut tasks: Vec<_> = p.tasks().collect();
        tasks.sort_by_key(|t| t.seq);
        tasks
            .into_iter()
            .enumerate()
            .map(|(i, t)| TaskModel {
                id: t.task.id,
                number: i + 1,
                title: t.task.title.clone(),
                status: Status::of(&t.state),
                live: p.task_is_live(t.task.id),
                part: t.task.part.clone().unwrap_or_else(|| "-".into()),
                raised_by: match t.task.raised_by {
                    Party::Builder => "You".into(),
                    Party::Agent(a) => p
                        .agent(a)
                        .map_or_else(|| "an agent".into(), |a| a.role.clone()),
                },
            })
            .collect()
    }

    pub fn read(p: &ProjectState, id: TaskId) -> Option<TaskModel> {
        Self::all(p).into_iter().find(|t| t.id == id)
    }
}

/// One task. Redraws only when what it shows has changed.
pub struct TaskRow {
    session: Entity<Session>,
    project: ProjectId,
    id: TaskId,
    renders: usize,
    model: Option<TaskModel>,
}

impl TaskRow {
    fn new(
        session: Entity<Session>,
        project: ProjectId,
        id: TaskId,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut row = Self {
            session,
            project,
            id,
            renders: 0,
            model: None,
        };
        row.refresh(cx);
        row
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let id = self.id;
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| TaskModel::read(p, id))
            .flatten();
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }

    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    pub fn shown(&self) -> Option<&TaskModel> {
        self.model.as_ref()
    }
}

/// The columns' shares of the width, after the idea's, which takes the rest.
const STATUS_W: f32 = 120.;
const PART_W: f32 = 150.;
const BY_W: f32 = 100.;

fn columns(
    idea: impl IntoElement,
    status: impl IntoElement,
    part: impl IntoElement,
    by: impl IntoElement,
) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .child(div().flex_1().min_w_0().child(idea))
        .child(div().flex_none().w(px(STATUS_W)).child(status))
        .child(div().flex_none().w(px(PART_W)).child(part))
        .child(div().flex_none().w(px(BY_W)).child(by))
}

impl Render for TaskRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let Some(m) = self.model.clone() else {
            return div().into_any_element();
        };
        let cell = |t: String| {
            div()
                .text_size(px(theme::SMALL))
                .text_color(theme::muted())
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(t)
        };
        columns(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(theme::TINY))
                        .text_color(theme::dim())
                        .child(format!("T-{:03}", m.number)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(theme::TEXT))
                        .text_color(theme::text())
                        .when(m.live, |d| d.child(widgets::dot(7., theme::live())))
                        // A long title, such as a brief's first line, stops at its column.
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(m.title.clone()),
                        ),
                ),
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(theme::SMALL))
                .text_color(m.status.colour())
                .child(widgets::dot(7., m.status.colour()))
                .child(m.status.label()),
            cell(m.part.clone()),
            cell(m.raised_by.clone()),
        )
        .h(px(ROW_HEIGHT))
        .border_b_1()
        .border_color(theme::line())
        .into_any_element()
    }
}

enum Form {
    Closed,
    Open {
        title: Entity<TextInput>,
        ask: Entity<TextInput>,
        part: Entity<TextInput>,
        _submit: Subscription,
    },
}

pub struct TasksView {
    session: Entity<Session>,
    project: ProjectId,
    rows: HashMap<TaskId, Entity<TaskRow>>,
    listed: Vec<TaskId>,
    form: Form,
    outcome: Option<Outcome>,
    _subscription: Subscription,
}

impl TasksView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project
                        && (changes.parts.contains(&ProjectPart::Tasks)
                            || changes.parts.contains(&ProjectPart::Team)
                            || changes.records.iter().any(|r| matches!(r, Record::Task(_))))
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
            rows: HashMap::new(),
            listed: Vec::new(),
            form: Form::Closed,
            outcome: None,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    /// The rows, in order.
    pub fn rows(&self) -> Vec<Entity<TaskRow>> {
        self.listed.iter().map(|t| self.rows[t].clone()).collect()
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<TaskId> = self
            .session
            .read(cx)
            .project(self.project, |p, _| {
                TaskModel::all(p).into_iter().map(|t| t.id).collect()
            })
            .unwrap_or_default();
        for id in &ids {
            if !self.rows.contains_key(id) {
                let (session, project, id) = (self.session.clone(), self.project, *id);
                self.rows
                    .insert(id, cx.new(|cx| TaskRow::new(session, project, id, cx)));
            }
        }
        for id in &ids {
            self.rows[id].update(cx, |r, cx| r.refresh(cx));
        }
        if ids != self.listed {
            self.listed = ids;
            cx.notify();
        }
    }

    fn toggle_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.form, Form::Open { .. }) {
            self.form = Form::Closed;
        } else {
            let title = cx.new(|cx| TextInput::new("What should be done", false, cx));
            let ask = cx.new(|cx| TextInput::new("Details, if the title is not enough", true, cx));
            let part = cx.new(|cx| TextInput::new("Part of the product, if any", false, cx));
            let submit = cx.subscribe(&title, |this, _, InputEvent::Submit, cx| this.raise(cx));
            window.focus(&gpui::Focusable::focus_handle(&title, cx), cx);
            self.form = Form::Open {
                title,
                ask,
                part,
                _submit: submit,
            };
            self.outcome = None;
        }
        cx.notify();
    }

    /// Raises the task the form describes.
    pub fn raise(&mut self, cx: &mut Context<Self>) {
        let Form::Open {
            title, ask, part, ..
        } = &self.form
        else {
            return;
        };
        let (title, ask, part) = (
            title.read(cx).text().trim().to_owned(),
            ask.read(cx).text().trim().to_owned(),
            part.read(cx).text().trim().to_owned(),
        );
        if title.is_empty() {
            self.outcome = Some(Outcome::Failed("Give the task a title.".into()));
            cx.notify();
            return;
        }
        let client = self.session.read(cx).client().clone();
        let project = self.project;
        self.outcome = Some(Outcome::Working);
        cx.notify();
        let ask = if ask.is_empty() { title.clone() } else { ask };
        notice::run(
            cx,
            async move {
                client
                    .raise_task(project, title, ask, (!part.is_empty()).then_some(part))
                    .await
            },
            |this, result, cx| {
                match result {
                    Ok(_) => {
                        this.form = Form::Closed;
                        this.outcome = None;
                    }
                    Err(outcome) => this.outcome = Some(outcome),
                }
                cx.notify();
            },
        );
    }

    fn form(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Form::Open {
            title, ask, part, ..
        } = &self.form
        else {
            return div().into_any_element();
        };
        let field = |id: &'static str, input: &Entity<TextInput>| {
            div()
                .debug_selector(move || id.into())
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme::line())
                .child(input.clone())
        };
        div()
            .mx(px(16.))
            .mb(px(12.))
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .rounded(px(12.))
            .border_1()
            .border_color(gpui::rgba(0xffffff60))
            .child(field("raise-title", title))
            .child(field("raise-ask", ask))
            .child(field("raise-part", part))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        glass::primary("raise-send", "Raise")
                            .selector("raise-send")
                            .on_click(cx.listener(|this, _, _, cx| this.raise(cx))),
                    )
                    .child(
                        widgets::button("raise-cancel", "Cancel")
                            .debug_selector(|| "raise-cancel".into())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_form(window, cx)),
                            ),
                    )
                    .children(self.outcome.as_ref().map(notice::line)),
            )
            .into_any_element()
    }
}

impl Render for TasksView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let head = |t: &'static str| {
            div()
                .text_size(px(theme::TINY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::dim())
                .child(t.to_uppercase())
        };
        div()
            .id("tasks")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_end()
                    .px(px(16.))
                    .py(px(10.))
                    .child(
                        widgets::button("raise-task", "Raise a task")
                            .debug_selector(|| "raise-task".into())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_form(window, cx)),
                            ),
                    ),
            )
            .child(self.form(cx))
            .child(
                columns(
                    head("Idea"),
                    head("Status"),
                    head("Part"),
                    head("Raised by"),
                )
                .py(px(8.))
                .border_y_1()
                .border_color(theme::line()),
            )
            .children(self.listed.iter().map(|id| {
                self.rows[id]
                    .clone()
                    .cached(StyleRefinement::default().w_full().h(px(ROW_HEIGHT)))
            }))
            .when(self.listed.is_empty(), |d| {
                d.child(
                    div()
                        .p(px(24.))
                        .text_size(px(theme::TEXT))
                        .text_color(theme::dim())
                        .child("No tasks yet. The CEO plans them once it has a brief."),
                )
            })
    }
}
