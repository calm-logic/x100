//! Settings, Decision model: which decision model labels the actions no rule covers. It shows
//! whether one is connected and which; the builder picks a provider, pastes its API key and
//! connects, or disconnects. The key goes to the daemon's vault once and is never shown again:
//! the field is masked and emptied once the daemon has it.

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::decision::DecisionProvider;
use x100_core::fold::DaemonState;
use x100_core::money::Percent;
use x100_core::settings::DaemonSetting;
use x100_proto::SetDaemonSetting;

use crate::glass;
use crate::input::{InputEvent, TextInput};
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, widgets, words};

/// What the part shows, read from the daemon's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionPartModel {
    /// The connected model, or `None`.
    pub connected: Option<DecisionProvider>,
    /// The daemon's confidence threshold, in percent: an answer less sure than this has its risk
    /// raised one level.
    pub threshold: u8,
}

/// The values offered, inside the range the daemon accepts (50 to 95).
const THRESHOLDS: [u8; 5] = [50, 60, 70, 80, 90];

impl DecisionPartModel {
    pub fn read(daemon: &DaemonState) -> DecisionPartModel {
        let settings = daemon.settings();
        DecisionPartModel {
            connected: settings.decision_model.map(|m| m.provider),
            threshold: settings.decision_threshold.value(),
        }
    }

    /// The values to offer: the usual ones, and the one in force when a script set another.
    fn offered(&self) -> Vec<u8> {
        let mut all = THRESHOLDS.to_vec();
        if !all.contains(&self.threshold) {
            all.push(self.threshold);
            all.sort_unstable();
        }
        all
    }
}

pub struct DecisionPart {
    session: Entity<Session>,
    model: DecisionPartModel,
    /// The provider a key is being entered for.
    chosen: DecisionProvider,
    key: Entity<TextInput>,
    outcome: Option<Outcome>,
    _submit: Subscription,
    _subscription: Subscription,
}

impl DecisionPart {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            if matches!(
                event,
                SessionEvent::Changed(Change::Daemon(_) | Change::Reset)
            ) {
                this.refresh(cx);
            }
        });
        let model = session
            .read(cx)
            .client()
            .with_state(|s| DecisionPartModel::read(&s.daemon));
        let (key, submit) = key_field(cx);
        Self {
            session,
            chosen: model.connected.unwrap_or(DecisionProvider::Gemini),
            model,
            key,
            outcome: None,
            _submit: submit,
            _subscription: subscription,
        }
    }

    pub fn shown(&self) -> &DecisionPartModel {
        &self.model
    }

    pub fn chosen(&self) -> DecisionProvider {
        self.chosen
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    /// What the key field holds now; empty once the daemon has taken a key.
    pub fn key_text(&self, cx: &gpui::App) -> String {
        self.key.read(cx).text().to_owned()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .client()
            .with_state(|s| DecisionPartModel::read(&s.daemon));
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }

    pub fn choose(&mut self, provider: DecisionProvider, cx: &mut Context<Self>) {
        self.chosen = provider;
        self.outcome = None;
        cx.notify();
    }

    /// Sends the key for the chosen provider. Once the daemon has it the field is replaced with
    /// an empty one, so the key is held nowhere in the app.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        let key = self.key.read(cx).text().trim().to_owned();
        if key.is_empty() {
            self.outcome = Some(Outcome::Failed("Paste the provider's API key.".into()));
            cx.notify();
            return;
        }
        let client = self.session.read(cx).client().clone();
        let provider = self.chosen;
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.connect_decision_model(provider, key).await },
            |this, result, cx| {
                match result {
                    Ok(_) => {
                        let (key, submit) = key_field(cx);
                        this.key = key;
                        this._submit = submit;
                        this.outcome = None;
                    }
                    Err(o) => this.outcome = Some(o),
                }
                cx.notify();
            },
        );
    }

    /// Sets the daemon's threshold. What the part shows changes when the daemon says it has.
    pub fn set_threshold(&mut self, percent: u8, cx: &mut Context<Self>) {
        let Ok(percent) = Percent::new(percent) else {
            return;
        };
        let client = self.session.read(cx).client().clone();
        self.outcome = Some(Outcome::Working);
        cx.notify();
        let request = async move {
            client
                .request(SetDaemonSetting {
                    setting: DaemonSetting::DecisionThreshold(percent),
                })
                .await
                .map(drop)
        };
        notice::run(cx, request, |this, result, cx| {
            this.outcome = result.err();
            cx.notify();
        });
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        let client = self.session.read(cx).client().clone();
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.disconnect_decision_model().await },
            |this, result, cx| {
                this.outcome = result.err();
                cx.notify();
            },
        );
    }
}

fn key_field(cx: &mut Context<DecisionPart>) -> (Entity<TextInput>, Subscription) {
    let key = cx.new(|cx| TextInput::new("API key", false, cx).masked());
    let submit = cx.subscribe(&key, |this, _, InputEvent::Submit, cx| this.connect(cx));
    (key, submit)
}

fn selector(provider: DecisionProvider) -> &'static str {
    match provider {
        DecisionProvider::Gemini => "decision-provider-gemini",
        DecisionProvider::OpenAi => "decision-provider-openai",
        DecisionProvider::Jev => "decision-provider-jev",
    }
}

impl Render for DecisionPart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (said, colour) = match self.model.connected {
            Some(p) => (
                format!(
                    "Connected: {}. It labels each action no rule covers, and its key is used \
                     by x100 alone, never by an agent. Each call is charged to the acting \
                     agent's budget.",
                    words::decision_provider(p)
                ),
                theme::muted(),
            ),
            None => (
                "Not connected. Nothing is classified: a read no rule covers goes through, and \
                 anything else no rule covers waits for you."
                    .to_owned(),
                theme::ask(),
            ),
        };
        let choices = DecisionProvider::ALL.into_iter().enumerate().map(|(i, p)| {
            widgets::segment(
                ("decision-provider", i),
                words::decision_provider(p),
                p == self.chosen,
            )
            .debug_selector(move || selector(p).to_owned())
            .on_click(cx.listener(move |this, _, _, cx| this.choose(p, cx)))
            .into_any_element()
        });
        let offered = self.model.offered();
        let thresholds = offered.clone().into_iter().map(|n| {
            widgets::segment(
                ("decision-threshold", usize::from(n)),
                format!("{n}%"),
                n == self.model.threshold,
            )
            .debug_selector(move || format!("decision-threshold-{n}"))
            .on_click(cx.listener(move |this, _, _, cx| this.set_threshold(n, cx)))
            .into_any_element()
        });
        let label = if self.model.connected.is_some() {
            "Connect instead"
        } else {
            "Connect"
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pt(px(14.))
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Decision model"),
            )
            .child(
                div()
                    .debug_selector(|| "decision-status".into())
                    .text_size(px(theme::SMALL))
                    .text_color(colour)
                    .child(said),
            )
            .child(widgets::segments(
                "decision-providers",
                DecisionProvider::ALL.iter().position(|p| *p == self.chosen),
                choices,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .text_size(px(theme::TEXT))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Raise the risk when it is less sure than"),
                    )
                    .child(widgets::segments(
                        "decision-thresholds",
                        offered.iter().position(|n| *n == self.model.threshold),
                        thresholds,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .debug_selector(|| "decision-key".into())
                            .flex_1()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(theme::line())
                            .child(self.key.clone()),
                    )
                    .child(
                        glass::primary("decision-connect", label)
                            .selector("decision-connect")
                            .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                    )
                    .when(self.model.connected.is_some(), |d| {
                        d.child(
                            widgets::button("decision-disconnect", "Disconnect")
                                .debug_selector(|| "decision-disconnect".into())
                                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx))),
                        )
                    }),
            )
            .children(self.outcome.as_ref().map(notice::line))
    }
}
