//! The Settings tab: the project's name, intent and code directory, which only the name of may be
//! changed here, Ask me about, the project's budget, each lead's budget and effort range, the
//! harnesses and models x100 may use, the Homebox project it deploys to, the Sandbox part with the
//! daemon's one unsafe setting, the Decision model part, the Integrations part, and the
//! Credentials part. Each control sends its protocol request and says beside itself what came
//! back: where the daemon has not made a method yet it says so, and the control does not pretend
//! to have worked. What a control changed shows when the state changes.

mod credentials;
mod decision;
mod homebox;
mod integrations;
mod sandbox;

use std::collections::HashMap;

use gpui::{AnyElement, Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::action::AskMeAbout;
use x100_core::agent::Position;
use x100_core::effort::{Effort, EffortRange};
use x100_core::fold::{DaemonState, ProjectPart, ProjectState};
use x100_core::harness::HarnessModel;
use x100_core::id::{AgentId, ProjectId};
use x100_core::money::Money;
use x100_core::project::SettingChange;

pub use credentials::{CredentialModel, Credentials};
pub use decision::{DecisionPart, DecisionPartModel};
pub use homebox::{HomeboxModel, HomeboxPart};
pub use integrations::{IntegrationsModel, IntegrationsPart};
pub use sandbox::{SandboxModel, SandboxPart};

use crate::glass;
use crate::input::{InputEvent, TextInput};
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::widgets::{self, row};
use crate::{theme, words};

const ASK: [AskMeAbout; 4] = [
    AskMeAbout::Nothing,
    AskMeAbout::HighRiskOnly,
    AskMeAbout::MediumAndUp,
    AskMeAbout::EveryDecision,
];
const SHORT: [&str; 4] = ["Low", "Med", "High", "Max"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadModel {
    pub id: AgentId,
    pub name: String,
    pub budget: Money,
    pub effort: EffortRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub what: HarnessModel,
    pub label: String,
    pub allowed: bool,
}

/// What the tab shows, read from the folded state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsModel {
    /// `None` until the CEO sets the project up.
    pub name: Option<String>,
    pub intent: Option<String>,
    pub code_dir: Option<String>,
    pub ask: AskMeAbout,
    pub budget: Money,
    pub leads: Vec<LeadModel>,
    pub models: Vec<ModelChoice>,
}

impl SettingsModel {
    pub fn read(p: &ProjectState, daemon: &DaemonState) -> Option<SettingsModel> {
        let project = p.project()?;
        let mut leads: Vec<_> = project
            .team
            .members()
            .filter_map(|(id, pos)| match pos {
                Position::Lead { effort, budget } => {
                    let a = p.agent(id)?;
                    Some((
                        a.joined,
                        LeadModel {
                            id,
                            name: a.role.clone(),
                            budget: *budget,
                            effort: *effort,
                        },
                    ))
                }
                _ => None,
            })
            .collect();
        leads.sort_by(|a, b| (a.0, &a.1.name).cmp(&(b.0, &b.1.name)));
        let models = daemon
            .harnesses()
            .flat_map(|h| {
                h.models.iter().map(|m| {
                    let what = HarnessModel {
                        harness: h.id,
                        model: m.model.clone(),
                    };
                    ModelChoice {
                        label: format!("{} \u{b7} {}", words::harness(daemon, h.id), m.model.0),
                        allowed: project.settings.allowed.contains(&what),
                        what,
                    }
                })
            })
            .collect();
        Some(SettingsModel {
            name: project.name().map(str::to_owned),
            intent: project.identity.as_ref().map(|i| i.intent.clone()),
            code_dir: project.code_dir.as_ref().map(|d| d.display().to_string()),
            ask: project.settings.ask_me_about,
            budget: project.settings.budget,
            leads: leads.into_iter().map(|(_, l)| l).collect(),
            models,
        })
    }
}

/// A control, to say what came back beside it and to find its text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    Name,
    Ask,
    Budget,
    LeadBudget(AgentId),
    Effort(AgentId),
    Model(usize),
}

pub struct SettingsView {
    session: Entity<Session>,
    project: ProjectId,
    model: Option<SettingsModel>,
    fields: HashMap<Control, (Entity<TextInput>, Subscription)>,
    outcomes: HashMap<Control, Outcome>,
    credentials: Entity<Credentials>,
    sandbox: Entity<SandboxPart>,
    decision: Entity<DecisionPart>,
    integrations: Entity<IntegrationsPart>,
    homebox: Entity<HomeboxPart>,
    _subscription: Subscription,
}

impl SettingsView {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Project { project, changes }) => {
                    *project == this.project
                        && [
                            ProjectPart::Settings,
                            ProjectPart::Team,
                            ProjectPart::Budget,
                        ]
                        .iter()
                        .any(|p| changes.parts.contains(p))
                }
                SessionEvent::Changed(Change::Daemon(_) | Change::Reset) => true,
                _ => false,
            };
            if ours {
                this.refresh(cx);
            }
        });
        let credentials = cx.new(|cx| Credentials::new(session.clone(), project, cx));
        let sandbox = cx.new(|cx| SandboxPart::new(session.clone(), cx));
        let decision = cx.new(|cx| DecisionPart::new(session.clone(), cx));
        let integrations = cx.new(|cx| IntegrationsPart::new(session.clone(), cx));
        let homebox = cx.new(|cx| HomeboxPart::new(session.clone(), project, cx));
        let mut view = Self {
            session,
            project,
            model: None,
            fields: HashMap::new(),
            outcomes: HashMap::new(),
            credentials,
            sandbox,
            decision,
            integrations,
            homebox,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    pub fn shown(&self) -> Option<&SettingsModel> {
        self.model.as_ref()
    }

    /// What came back beside a control, if anything.
    pub fn outcome(&self, control: Control) -> Option<&Outcome> {
        self.outcomes.get(&control)
    }

    pub fn credentials(&self) -> &Entity<Credentials> {
        &self.credentials
    }

    pub fn sandbox(&self) -> &Entity<SandboxPart> {
        &self.sandbox
    }

    pub fn decision(&self) -> &Entity<DecisionPart> {
        &self.decision
    }

    pub fn integrations(&self) -> &Entity<IntegrationsPart> {
        &self.integrations
    }

    pub fn homebox(&self) -> &Entity<HomeboxPart> {
        &self.homebox
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .project(self.project, SettingsModel::read)
            .flatten();
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }

    /// Sends a request for `control` and keeps what came back beside it.
    fn send(
        &mut self,
        control: Control,
        request: impl std::future::Future<Output = Result<(), x100_client::ClientError>> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.outcomes.insert(control, Outcome::Working);
        cx.notify();
        notice::run(cx, request, move |this, result, cx| {
            match result {
                // The state changes when the daemon has done it, and the control follows.
                Ok(()) => this.outcomes.remove(&control),
                Err(outcome) => this.outcomes.insert(control, outcome),
            };
            cx.notify();
        });
    }

    fn client(&self, cx: &Context<Self>) -> x100_client::Client {
        self.session.read(cx).client().clone()
    }

    pub fn set_ask(&mut self, level: AskMeAbout, cx: &mut Context<Self>) {
        let (client, project) = (self.client(cx), self.project);
        self.send(
            Control::Ask,
            async move {
                client
                    .change_setting(project, SettingChange::AskMeAbout(level))
                    .await
            },
            cx,
        );
    }

    pub fn allow(&mut self, index: usize, allowed: bool, cx: &mut Context<Self>) {
        let Some(choice) = self.model.as_ref().and_then(|m| m.models.get(index)) else {
            return;
        };
        let what = choice.what.clone();
        let (client, project) = (self.client(cx), self.project);
        self.send(
            Control::Model(index),
            async move {
                client
                    .change_setting(project, SettingChange::Allow { what, allowed })
                    .await
            },
            cx,
        );
    }

    /// Sets the range's low or high end, moving the other with it if they would cross.
    pub fn set_effort(
        &mut self,
        lead: AgentId,
        low: Option<Effort>,
        high: Option<Effort>,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self
            .model
            .as_ref()
            .and_then(|m| m.leads.iter().find(|l| l.id == lead))
            .map(|l| l.effort)
        else {
            return;
        };
        let (min, max) = (low.unwrap_or(range.min()), high.unwrap_or(range.max()));
        let (min, max) = match (low, high) {
            (Some(l), None) if l > max => (l, l),
            (None, Some(h)) if h < min => (h, h),
            _ => (min, max),
        };
        let Ok(range) = EffortRange::new(min, max) else {
            return;
        };
        let (client, project) = (self.client(cx), self.project);
        self.send(
            Control::Effort(lead),
            async move { client.set_effort_range(project, lead, range).await },
            cx,
        );
    }

    /// Renames the project to what the name's field holds.
    pub fn rename(&mut self, cx: &mut Context<Self>) {
        let Some((field, _)) = self.fields.get(&Control::Name) else {
            return;
        };
        // An empty name is the daemon's to refuse, and its answer shows beside the field.
        let name = field.read(cx).text().trim().to_owned();
        let (client, project) = (self.client(cx), self.project);
        self.send(
            Control::Name,
            async move { client.rename_project(project, name).await },
            cx,
        );
    }

    /// Sets the budget `control` names to what its field holds, or the name.
    pub fn apply_budget(&mut self, control: Control, cx: &mut Context<Self>) {
        if control == Control::Name {
            return self.rename(cx);
        }
        let Some((field, _)) = self.fields.get(&control) else {
            return;
        };
        let Some(money) = words::parse_money(field.read(cx).text()) else {
            self.outcomes.insert(
                control,
                Outcome::Failed("Enter an amount in dollars, such as 25 or 12.50.".into()),
            );
            cx.notify();
            return;
        };
        let (client, project) = (self.client(cx), self.project);
        match control {
            Control::Budget => self.send(
                control,
                async move {
                    client
                        .change_setting(project, SettingChange::Budget(money))
                        .await
                },
                cx,
            ),
            Control::LeadBudget(lead) => self.send(
                control,
                async move { client.set_lead_budget(project, lead, money).await },
                cx,
            ),
            _ => {}
        }
    }

    fn field(
        &mut self,
        control: Control,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some((field, _)) = self.fields.get(&control) {
            return field.clone();
        }
        let field = cx.new(|cx| TextInput::new(placeholder, false, cx));
        let submit = cx.subscribe(&field, move |this, _, InputEvent::Submit, cx| {
            this.apply_budget(control, cx)
        });
        self.fields.insert(control, (field.clone(), submit));
        field
    }

    /// A budget's field and its Set button, with what came back.
    fn amount(&mut self, control: Control, id: String, cx: &mut Context<Self>) -> AnyElement {
        let placeholder = match control {
            Control::Name => "A new name",
            _ => "New amount, in dollars",
        };
        let field = self.field(control, placeholder, cx);
        let (field_id, set_id) = (format!("{id}-field"), format!("{id}-set"));
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(self.outcomes.get(&control).map(notice::line))
            .child(
                div()
                    .debug_selector(move || field_id.clone())
                    .w(px(190.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme::line())
                    .child(field),
            )
            .child(
                glass::primary(set_id.clone(), "Set")
                    .selector(set_id.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.apply_budget(control, cx))),
            )
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(16.))
            .pb(px(16.));
        let Some(m) = self.model.clone() else {
            return page;
        };
        let outcome = |s: &Self, c: Control| s.outcomes.get(&c).map(notice::line);
        let text = |t: Option<String>, none: &str| {
            div()
                .text_size(px(theme::SMALL))
                .text_color(theme::muted())
                .child(t.unwrap_or_else(|| none.to_owned()))
        };
        // The name and intent are the CEO's, set when it sets the project up; the builder may
        // rename it after that. The code directory never moves.
        let name = match &m.name {
            Some(name) => row(
                "Name",
                Some(format!("Now {name}")),
                self.amount(Control::Name, "name".into(), cx),
            ),
            None => row(
                "Name",
                None,
                text(
                    None,
                    "None yet: the CEO names the project once it knows what it is",
                ),
            ),
        };
        let intent = row("Intent", None, text(m.intent.clone(), "None yet"));
        let code = row(
            "Code directory",
            None,
            text(
                m.code_dir.clone(),
                "None yet: made when the project is set up",
            ),
        );

        let ask = row(
            "Ask me about",
            None,
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .children(outcome(self, Control::Ask))
                .child(widgets::segments(
                    "ask",
                    ASK.iter().position(|l| *l == m.ask),
                    ASK.into_iter().enumerate().map(|(i, level)| {
                        widgets::segment(("ask", i), words::ask_me_about(level), level == m.ask)
                            .debug_selector(move || format!("ask-{i}"))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_ask(level, cx)))
                            .into_any_element()
                    }),
                )),
        );
        let total = self.amount(Control::Budget, "budget".into(), cx);
        let budget = row(
            "Project budget, in total",
            Some(format!("Now {}", words::money(m.budget))),
            total,
        );
        let mut leads = Vec::new();
        for lead in &m.leads {
            let id = lead.id;
            let amount = self.amount(
                Control::LeadBudget(id),
                format!("lead-budget-{}", lead.name),
                cx,
            );
            leads.push(
                row(
                    format!("{} budget", lead.name),
                    Some(format!("Now {}", words::money(lead.budget))),
                    amount,
                )
                .into_any_element(),
            );
            let name = lead.name.clone();
            let end = |high: bool, cx: &mut Context<Self>| {
                let name = name.clone();
                let at = if high {
                    lead.effort.max()
                } else {
                    lead.effort.min()
                };
                let key = if high { "hi" } else { "lo" };
                let group = format!("effort-{key}-{name}");
                widgets::segments(
                    gpui::ElementId::Name(group.into()),
                    Effort::ALL.iter().position(|e| *e == at),
                    Effort::ALL.into_iter().enumerate().map(|(i, e)| {
                        let on = if high {
                            lead.effort.max() == e
                        } else {
                            lead.effort.min() == e
                        };
                        let key = if high { "hi" } else { "lo" };
                        let selector = format!("effort-{key}-{name}-{i}");
                        widgets::segment(
                            gpui::ElementId::NamedInteger(selector.clone().into(), i as u64),
                            SHORT[i],
                            on,
                        )
                        .debug_selector(move || selector.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if high {
                                this.set_effort(id, None, Some(e), cx)
                            } else {
                                this.set_effort(id, Some(e), None, cx)
                            }
                        }))
                        .into_any_element()
                    }),
                )
            };
            leads.push(
                row(
                    format!("{} effort", lead.name),
                    None,
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .children(outcome(self, Control::Effort(id)))
                        .child(end(false, cx))
                        .child(
                            div()
                                .text_size(px(theme::SMALL))
                                .text_color(theme::muted())
                                .child("to"),
                        )
                        .child(end(true, cx)),
                )
                .into_any_element(),
            );
        }
        let models = m.models.iter().enumerate().map(|(i, choice)| {
            let on = choice.allowed;
            row(
                choice.label.clone(),
                None,
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .children(outcome(self, Control::Model(i)))
                    .child(widgets::segments(
                        ("model-switch", i),
                        on.then_some(0),
                        [
                            widgets::segment(("model", i), if on { "Allowed" } else { "Off" }, on)
                                .debug_selector(move || format!("model-{i}"))
                                .on_click(cx.listener(move |this, _, _, cx| this.allow(i, !on, cx)))
                                .into_any_element(),
                        ],
                    )),
            )
        });
        page.child(name)
            .child(intent)
            .child(code)
            .child(ask)
            .child(budget)
            .children(leads)
            .child(
                div()
                    .pt(px(14.))
                    .text_size(px(theme::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Harnesses and models x100 may use"),
            )
            .children(models)
            .child(
                div()
                    .pt(px(10.))
                    .text_size(px(theme::TINY))
                    .text_color(theme::dim())
                    .child("The CEO sets each lead's effort range. For every task x100 picks the cheapest allowed model that meets the effort the task needs."),
            )
            .child(self.homebox.clone())
            .child(self.sandbox.clone())
            .child(self.decision.clone())
            .child(self.integrations.clone())
            .child(div().pt(px(18.)).child(self.credentials.clone()))
    }
}
