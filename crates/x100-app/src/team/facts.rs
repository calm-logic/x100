//! What an agent's view shows beside its output: the drivers it has had and why each ended, what
//! it is waiting for with each deadline, the jobs it has scheduled with a cancel on each, and how
//! many items wait in its inbox. A cached view of its own, redrawn only when what it shows
//! changes, so output arriving below never redraws it.

use std::collections::HashSet;

use gpui::{Context, Entity, FontWeight, SharedString, Window, div, prelude::*, px};
use x100_core::driver::Purpose;
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{AgentId, JobId, ProjectId};
use x100_core::job::{JobState, ProjectJob};
use x100_core::party::Party;
use x100_core::wait::WaitOn;
use x100_proto::CancelJob;

use super::log;
use crate::session::Session;
use crate::{theme, widgets};

pub const FACTS_HEIGHT: f32 = 104.;
/// How many lines each column shows before it says how many more there are.
const SHOWN: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLine {
    pub job: JobId,
    pub text: String,
}

/// What the facts show, read from the folded state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FactsModel {
    /// Newest first: "2 · fake-fernleaf · fake-brief · Low · Running".
    pub drivers: Vec<String>,
    pub waits: Vec<String>,
    pub jobs: Vec<JobLine>,
    pub inbox: usize,
}

impl FactsModel {
    pub fn read(p: &ProjectState, daemon: &DaemonState, agent: AgentId) -> Option<FactsModel> {
        let a = p.agent(agent)?;
        let role = |id: AgentId| {
            p.agent(id)
                .map_or_else(|| "an agent".into(), |r| r.role.clone())
        };
        let mut drivers: Vec<String> = a
            .drivers
            .iter()
            .filter_map(|d| p.driver(*d))
            .filter(|d| d.driver.purpose == Purpose::Work)
            .enumerate()
            .map(|(i, d)| {
                let state = log::ended(&d.state)
                    .unwrap_or_else(|| crate::words::driver_state(&d.state).into());
                format!(
                    "{} · {} · {state}",
                    i + 1,
                    log::driver_label(daemon, &d.driver)
                )
            })
            .collect();
        drivers.reverse();
        let waits = a
            .waits
            .values()
            .map(|w| {
                let on = match w.on {
                    WaitOn::Job(job) => match p.job(job).map(|j| &j.job.does) {
                        Some(ProjectJob::Wake { note, .. }) => format!("its job: {note}"),
                        _ => "a job".into(),
                    },
                    WaitOn::Report(task) => match p.task(task) {
                        Some(t) => format!("a report on “{}”", t.task.title),
                        None => "a report".into(),
                    },
                    WaitOn::Reply { from, .. } => match from {
                        Party::Builder => "the builder's answer".into(),
                        Party::Agent(id) => format!("a reply from {}", role(id)),
                    },
                    WaitOn::Decision(_) => "a decision on an action".into(),
                };
                format!("{on}, until {}", crate::words::moment(w.deadline))
            })
            .collect();
        let jobs = p
            .jobs_of(agent)
            .map(|j| {
                let what = match &j.job.does {
                    ProjectJob::Wake {
                        agent: who, note, ..
                    } if *who == agent => {
                        format!("Wake itself: {note}")
                    }
                    ProjectJob::Wake {
                        agent: who, note, ..
                    } => {
                        format!("Wake {}: {note}", role(*who))
                    }
                    ProjectJob::Delegate { to, title, .. } => {
                        format!("“{title}” to {}", role(*to))
                    }
                    ProjectJob::Routine(_) => "A routine".into(),
                };
                let when = match j.state {
                    JobState::Scheduled {
                        due,
                        every: Some(every),
                    } => format!(
                        "next {}, every {}",
                        crate::words::moment(due),
                        interval(every.get())
                    ),
                    JobState::Scheduled { due, every: None } => {
                        format!("at {}", crate::words::moment(due))
                    }
                    JobState::Done | JobState::Cancelled => String::new(),
                };
                JobLine {
                    job: j.job.id,
                    text: format!("{what} · {when}"),
                }
            })
            .collect();
        Some(FactsModel {
            drivers,
            waits,
            jobs,
            inbox: a.inbox.unhandled().count(),
        })
    }
}

fn interval(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    match secs {
        86_400 => "day".into(),
        3_600 => "hour".into(),
        s if s.is_multiple_of(86_400) => format!("{} days", s / 86_400),
        s if s.is_multiple_of(3_600) => format!("{} hours", s / 3_600),
        s => format!("{} minutes", s / 60),
    }
}

pub struct AgentFacts {
    session: Entity<Session>,
    project: ProjectId,
    agent: AgentId,
    model: Option<FactsModel>,
    /// Jobs whose cancel has been sent and not yet seen in the state.
    cancelling: HashSet<JobId>,
    error: Option<String>,
    renders: usize,
}

impl AgentFacts {
    pub fn new(
        session: Entity<Session>,
        project: ProjectId,
        agent: AgentId,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut facts = Self {
            session,
            project,
            agent,
            model: None,
            cancelling: HashSet::new(),
            error: None,
            renders: 0,
        };
        facts.refresh(cx);
        facts
    }

    /// Reads what it shows now, and redraws only if that changed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let agent = self.agent;
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, d| FactsModel::read(p, d, agent))
            .flatten();
        if now != self.model {
            if let Some(m) = &now {
                self.cancelling
                    .retain(|job| m.jobs.iter().any(|l| l.job == *job));
            }
            self.model = now;
            cx.notify();
        }
    }

    pub fn shown(&self) -> Option<&FactsModel> {
        self.model.as_ref()
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// Asks the daemon to cancel a job, as its Cancel button does.
    pub fn cancel(&mut self, job: JobId, cx: &mut Context<Self>) {
        if !self.cancelling.insert(job) {
            return;
        }
        self.error = None;
        cx.notify();
        let client = self.session.read(cx).client().clone();
        let project = self.project;
        cx.spawn(async move |this, cx| {
            let done = client.request(CancelJob { project, job }).await;
            let _ = this.update(cx, |f, cx| {
                if let Err(e) = done {
                    f.cancelling.remove(&job);
                    f.error = Some(format!("The job was not cancelled: {e}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn column(
        title: &'static str,
        share: f32,
        lines: Vec<gpui::AnyElement>,
        more: usize,
    ) -> gpui::Div {
        div()
            .flex_grow(share)
            .flex_basis(px(0.))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(px(9.))
                    .text_color(theme::dim())
                    .child(title),
            )
            .children(lines)
            .when(more > 0, |d| {
                d.child(
                    div()
                        .text_color(theme::dim())
                        .child(format!("and {more} more")),
                )
            })
    }
}

fn line(text: impl Into<SharedString>) -> gpui::AnyElement {
    div()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_ellipsis()
        .child(text.into())
        .into_any_element()
}

impl Render for AgentFacts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let m = self.model.clone().unwrap_or_default();
        let none = |text: &'static str| vec![line(text)];
        let drivers = if m.drivers.is_empty() {
            none("None yet")
        } else {
            m.drivers
                .iter()
                .take(SHOWN)
                .map(|d| line(d.clone()))
                .collect()
        };
        let waits = if m.waits.is_empty() {
            none("Nothing")
        } else {
            m.waits
                .iter()
                .take(SHOWN)
                .map(|w| line(w.clone()))
                .collect()
        };
        let jobs = if m.jobs.is_empty() {
            none("None")
        } else {
            m.jobs
                .iter()
                .take(SHOWN)
                .enumerate()
                .map(|(i, j)| {
                    let job = j.job;
                    let sent = self.cancelling.contains(&job);
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .min_w_0()
                        .child(div().flex_1().min_w_0().child(line(j.text.clone())))
                        .child(if sent {
                            div()
                                .text_color(theme::dim())
                                .child("Cancelling…")
                                .into_any_element()
                        } else {
                            widgets::button(("cancel-job", i), "Cancel")
                                .debug_selector(move || format!("cancel-job-{i}"))
                                .py(px(1.))
                                .px(px(8.))
                                .text_size(px(theme::TINY))
                                .on_click(cx.listener(move |this, _, _, cx| this.cancel(job, cx)))
                                .into_any_element()
                        })
                        .into_any_element()
                })
                .collect()
        };
        div()
            .h(px(FACTS_HEIGHT))
            .w_full()
            .flex()
            .gap(px(16.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(theme::line())
            .text_size(px(theme::TINY))
            .text_color(theme::muted())
            .child(Self::column(
                "DRIVERS",
                1.6,
                drivers,
                m.drivers.len().saturating_sub(SHOWN),
            ))
            .child(Self::column(
                "WAITING FOR",
                1.2,
                waits,
                m.waits.len().saturating_sub(SHOWN),
            ))
            .child(
                Self::column("JOBS", 1.2, jobs, m.jobs.len().saturating_sub(SHOWN))
                    .when_some(self.error.clone(), |d, e| {
                        d.child(div().text_color(theme::error()).child(e))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(theme::dim())
                            .child("INBOX"),
                    )
                    .child(
                        div()
                            .text_size(px(theme::ROW_TEXT))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text())
                            .child(m.inbox.to_string()),
                    ),
            )
    }
}
