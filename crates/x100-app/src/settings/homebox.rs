//! Settings, Homebox project: the project of the connected Homebox this project deploys to,
//! picked from that Homebox's projects, and Unlink. The CEO can link one too, when the builder
//! asks it to in the chat; either way the link shows here.

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::ProjectPart;
use x100_core::id::ProjectId;
use x100_core::integration::HomeboxLink;
use x100_proto::HomeboxProject;

use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, widgets};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HomeboxModel {
    /// Whether a Homebox is connected, and at what address.
    pub connected: Option<String>,
    pub link: Option<HomeboxLink>,
    /// The connected Homebox's projects, once it has answered.
    pub projects: Option<Vec<HomeboxProject>>,
}

pub struct HomeboxPart {
    session: Entity<Session>,
    project: ProjectId,
    model: HomeboxModel,
    outcome: Option<Outcome>,
    _subscription: Subscription,
}

impl HomeboxPart {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project && changes.parts.contains(&ProjectPart::Settings)
                }
                SessionEvent::Changed(Change::Daemon(_) | Change::Reset) => true,
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let mut part = Self {
            session,
            project,
            model: HomeboxModel::default(),
            outcome: None,
            _subscription: subscription,
        };
        part.refresh(cx);
        part
    }

    pub fn shown(&self) -> &HomeboxModel {
        &self.model
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn client(&self, cx: &Context<Self>) -> x100_client::Client {
        self.session.read(cx).client().clone()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let project = self.project;
        let (connected, link) = self.client(cx).with_state(|s| {
            let connected = s
                .daemon
                .settings()
                .homebox
                .as_ref()
                .map(|c| c.address.clone());
            let link = s
                .projects
                .get(&project)
                .and_then(|p| p.project())
                .and_then(|p| p.homebox.clone());
            (connected, link)
        });
        if (&connected, &link) == (&self.model.connected, &self.model.link) {
            return;
        }
        let another = connected != self.model.connected;
        self.model.connected = connected;
        self.model.link = link;
        // Another Homebox has other projects: ask it.
        if another {
            self.model.projects = None;
            if self.model.connected.is_some() {
                self.load(cx);
            }
        }
        cx.notify();
    }

    /// Asks the daemon for the connected Homebox's projects.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let client = self.client(cx);
        notice::run(
            cx,
            async move { client.homebox_projects().await },
            |this, result, cx| {
                match result {
                    Ok(listed) => this.model.projects = Some(listed.projects),
                    Err(o) => this.outcome = Some(o),
                }
                cx.notify();
            },
        );
    }

    pub fn link(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(chosen) = self
            .model
            .projects
            .as_ref()
            .and_then(|all| all.get(index))
            .map(|p| p.repo_full_name.clone())
        else {
            return;
        };
        let (client, project) = (self.client(cx), self.project);
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.link_homebox(project, chosen, None).await },
            |this, result, cx| {
                this.outcome = result.err();
                cx.notify();
            },
        );
    }

    pub fn unlink(&mut self, cx: &mut Context<Self>) {
        let (client, project) = (self.client(cx), self.project);
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.unlink_homebox(project).await },
            |this, result, cx| {
                this.outcome = result.err();
                cx.notify();
            },
        );
    }
}

impl Render for HomeboxPart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &self.model;
        let said = match (&m.connected, &m.link) {
            (None, None) => {
                "Not linked. Connect Homebox under Integrations to deploy there.".into()
            }
            (_, Some(l)) => format!(
                "Deploys to {} ({}), {}.",
                l.project_name, l.repo_full_name, l.environment
            ),
            (Some(_), None) => "Not linked. Pick the Homebox project it deploys to, or ask the \
                                CEO to deploy it and x100 makes one."
                .into(),
        };
        let linked = m.link.as_ref().map(|l| l.repo_full_name.clone());
        let managed: Vec<_> = m
            .projects
            .iter()
            .flatten()
            .enumerate()
            .filter(|(_, p)| p.managed)
            .collect();
        let chosen = managed
            .iter()
            .position(|(_, p)| linked.as_deref() == Some(p.repo_full_name.as_str()));
        let choices = managed
            .into_iter()
            .map(|(i, p)| {
                let on = linked.as_deref() == Some(p.repo_full_name.as_str());
                let selector = format!("homebox-project-{}", p.name);
                widgets::segment(("homebox-project", i), p.name.clone(), on)
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.link(i, cx)))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .py(px(12.))
            .border_b_1()
            .border_color(theme::line())
            .child(
                div()
                    .text_size(px(theme::TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Homebox project"),
            )
            .child(
                div()
                    .debug_selector(|| "homebox-link".into())
                    .text_size(px(theme::SMALL))
                    .text_color(theme::muted())
                    .child(said),
            )
            .when(!choices.is_empty(), |d| {
                d.child(widgets::segments("homebox-projects", chosen, choices))
            })
            .when(m.link.is_some(), |d| {
                d.child(
                    widgets::button("homebox-unlink", "Unlink")
                        .debug_selector(|| "homebox-unlink".into())
                        .on_click(cx.listener(|this, _, _, cx| this.unlink(cx))),
                )
            })
            .children(self.outcome.as_ref().map(notice::line))
    }
}
