//! Settings, Reflection: the project's "trial before commit" setting, and Reflect now, which gives
//! the CEO a task to look back on the last piece of work. Each says beside itself what the daemon
//! answered: Reflect now the task it opened, or why it would not.

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::{ProjectId, TaskId};

use crate::glass;
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, widgets};

/// What Reflect now answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reflected {
    Said(Outcome),
    /// The daemon opened this task for the CEO.
    Opened(TaskId),
}

pub struct ReflectionPart {
    session: Entity<Session>,
    project: ProjectId,
    /// The project's setting, `None` until the client holds the project.
    trial_before_commit: Option<bool>,
    outcome: Option<Outcome>,
    reflected: Option<Reflected>,
    _subscription: Subscription,
}

impl ReflectionPart {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project
                        && (changes.parts.contains(&ProjectPart::Settings)
                            || changes.parts.contains(&ProjectPart::Tasks))
                }
                SessionEvent::Changed(Change::Reset) => true,
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let mut part = Self {
            session,
            project,
            trial_before_commit: None,
            outcome: None,
            reflected: None,
            _subscription: subscription,
        };
        part.refresh(cx);
        part
    }

    /// The setting as the tab shows it.
    pub fn trial_before_commit(&self) -> Option<bool> {
        self.trial_before_commit
    }

    /// The line under the setting, saying what it means as it stands.
    pub fn meaning(&self) -> &'static str {
        if self.trial_before_commit.unwrap_or(false) {
            "On: each skill change or memory compaction reflection proposes is tried in the lab \
             first, and kept only if it helps. Corrections of notes are never put on trial."
        } else {
            "Off: reflection's changes are committed directly, and can be reverted. Corrections \
             of notes are never put on trial."
        }
    }

    /// What came back beside the setting.
    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// What came back beside Reflect now.
    pub fn reflected(&self) -> Option<&Reflected> {
        self.reflected.as_ref()
    }

    fn client(&self, cx: &Context<Self>) -> x100_client::Client {
        self.session.read(cx).client().clone()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .project(self.project, |p, _| {
                p.project().map(|p| p.settings.trial_before_commit)
            })
            .flatten();
        self.trial_before_commit = now;
        // The opened task's title is read when drawn, so a change to the tasks redraws too.
        cx.notify();
    }

    pub fn set_trial_before_commit(&mut self, on: bool, cx: &mut Context<Self>) {
        let (client, project) = (self.client(cx), self.project);
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.set_trial_before_commit(project, on).await },
            |this, result, cx| {
                // The state changes when the daemon has done it, and the switch follows.
                this.outcome = result.err();
                cx.notify();
            },
        );
    }

    pub fn reflect(&mut self, cx: &mut Context<Self>) {
        let (client, project) = (self.client(cx), self.project);
        self.reflected = Some(Reflected::Said(Outcome::Working));
        cx.notify();
        notice::run(
            cx,
            async move { client.reflect(project).await },
            |this, result, cx| {
                this.reflected = Some(result.map_or_else(Reflected::Said, Reflected::Opened));
                cx.notify();
            },
        );
    }

    /// The line beside Reflect now once a task was opened, with its title once the client holds
    /// it.
    pub fn opened_line(&self, task: TaskId, cx: &gpui::App) -> String {
        let title = self
            .session
            .read(cx)
            .project(self.project, |p, _| {
                p.task(task).map(|t| t.task.title.clone())
            })
            .flatten();
        match title {
            Some(title) => format!("Opened \u{201c}{title}\u{201d} for the CEO."),
            None => "Opened a reflection task for the CEO.".into(),
        }
    }
}

impl Render for ReflectionPart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.trial_before_commit.unwrap_or(false);
        let meaning = self.meaning();
        let reflected = match &self.reflected {
            Some(Reflected::Said(o)) => Some(notice::line(o)),
            Some(Reflected::Opened(task)) => Some(
                div()
                    .text_size(px(theme::TINY))
                    .text_color(theme::muted())
                    .child(self.opened_line(*task, cx)),
            ),
            None => None,
        };
        let label = |text: &'static str| {
            div()
                .text_size(px(theme::TEXT))
                .font_weight(FontWeight::MEDIUM)
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(theme::line())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(16.))
                            .child(label("Trial before commit"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .children(self.outcome.as_ref().map(notice::line))
                                    .child(widgets::segments(
                                        "trial-before-commit-switch",
                                        on.then_some(0),
                                        [widgets::segment(
                                            "trial-before-commit",
                                            if on { "On" } else { "Off" },
                                            on,
                                        )
                                        .debug_selector(|| "trial-before-commit".to_owned())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_trial_before_commit(!on, cx)
                                        }))
                                        .into_any_element()],
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .debug_selector(|| "trial-before-commit-meaning".to_owned())
                            .text_size(px(theme::SMALL))
                            .text_color(theme::muted())
                            .child(meaning),
                    ),
            )
            .child(widgets::row(
                "Reflect now",
                Some("The CEO looks back on the work since it last reflected.".into()),
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .children(reflected)
                    .child(
                        glass::primary("reflect-now", "Reflect now")
                            .selector("reflect-now")
                            .on_click(cx.listener(|this, _, _, cx| this.reflect(cx))),
                    ),
            ))
    }
}
