//! Settings, Sandbox: whether drivers run contained, and the daemon's one setting that runs them
//! without a sandbox, which is shown as unsafe whenever it is on. Where no sandbox can be made on
//! the daemon's machine, it says why, since no driver starts there until the setting is on.

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::DaemonState;
use x100_core::settings::DaemonSetting;
use x100_proto::SetDaemonSetting;

use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, widgets};

/// What the part shows, read from the daemon's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxModel {
    /// The daemon's `run_without_sandbox` setting.
    pub uncontained: bool,
    /// Why no sandbox can be made on the daemon's machine, if none can.
    pub missing: Option<String>,
}

impl SandboxModel {
    pub fn read(daemon: &DaemonState) -> SandboxModel {
        let missing = daemon
            .machines()
            .flat_map(|m| m.passed_over.iter())
            .find(|p| p.why.starts_with("no sandbox can be made"))
            .map(|p| p.why.clone());
        SandboxModel {
            uncontained: daemon.settings().run_without_sandbox,
            missing,
        }
    }
}

pub struct SandboxPart {
    session: Entity<Session>,
    model: SandboxModel,
    outcome: Option<Outcome>,
    _subscription: Subscription,
}

impl SandboxPart {
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
            .with_state(|s| SandboxModel::read(&s.daemon));
        Self {
            session,
            model,
            outcome: None,
            _subscription: subscription,
        }
    }

    pub fn shown(&self) -> &SandboxModel {
        &self.model
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = self
            .session
            .read(cx)
            .client()
            .with_state(|s| SandboxModel::read(&s.daemon));
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }

    /// Turns the daemon's `run_without_sandbox` setting on or off.
    pub fn set_uncontained(&mut self, on: bool, cx: &mut Context<Self>) {
        let client = self.session.read(cx).client().clone();
        self.outcome = Some(Outcome::Working);
        cx.notify();
        let request = async move {
            client
                .request(SetDaemonSetting {
                    setting: DaemonSetting::RunWithoutSandbox(on),
                })
                .await
                .map(drop)
        };
        notice::run(cx, request, |this, result, cx| {
            this.outcome = result.err();
            cx.notify();
        });
    }
}

impl Render for SandboxPart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &self.model;
        let (said, colour) = match (m.uncontained, &m.missing) {
            (true, _) => (
                "Unsafe: drivers run without a sandbox, as you, with your files, your logins and \
                 your network. Anything an agent runs can read or change them."
                    .to_owned(),
                theme::error(),
            ),
            (false, Some(why)) => (why.clone(), theme::ask()),
            (false, None) => (
                "Every agent's drivers run in a sandbox of its own, which shows them their work \
                 and nothing else of yours, and reach the network only through x100's proxy."
                    .to_owned(),
                theme::muted(),
            ),
        };
        let on = m.uncontained;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pt(px(14.))
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Sandbox"),
            )
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .text_color(colour)
                    .child(said),
            )
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
                            .text_color(if on { theme::error() } else { theme::text() })
                            .child("Run drivers without a sandbox (unsafe)"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .children(self.outcome.as_ref().map(notice::line))
                            .child(widgets::segments([widgets::segment(
                                "uncontained",
                                if on { "On, unsafe" } else { "Off" },
                                on,
                            )
                            .debug_selector(|| "run-without-sandbox".to_owned())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.set_uncontained(!on, cx)),
                            )
                            .into_any_element()])),
                    ),
            )
    }
}
