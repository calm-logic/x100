//! Settings, Integrations: Homebox, where projects are deployed, and GitHub, where their code is
//! hosted so Homebox can deploy it. Each shows whether it is connected and how, with the way the
//! daemon found on this machine chosen first and the other one click away, and Disconnect.
//! Homebox's "Use local" asks before the daemon runs its one command in Homebox's container. A
//! pasted key goes to the daemon's vault once and the field is emptied, as for a decision model.

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::fold::DaemonState;
use x100_core::integration::{GitHubConnection, GitHubWay, HomeboxConnection, HomeboxWay};
use x100_proto::{GitHubConnecting, GitHubMethod, Integration, IntegrationList};

use crate::input::{InputEvent, TextInput};
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::{theme, widgets};

/// What the part shows: the connections, from the daemon's settings, and what the daemon found
/// on its machine, from `integrations.list`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrationsModel {
    pub homebox: Option<HomeboxConnection>,
    pub github: Option<GitHubConnection>,
    /// `None` until the daemon has said.
    pub found: Option<IntegrationList>,
}

impl IntegrationsModel {
    fn read(daemon: &DaemonState, found: Option<IntegrationList>) -> IntegrationsModel {
        IntegrationsModel {
            homebox: daemon.settings().homebox.clone(),
            github: daemon.settings().github.clone(),
            found,
        }
    }
}

pub struct IntegrationsPart {
    session: Entity<Session>,
    model: IntegrationsModel,
    /// The way chosen for Homebox; the daemon's default until the builder picks.
    homebox_way: Option<HomeboxWay>,
    /// "Use local" was pressed, and the builder is asked before the key is minted.
    confirming: bool,
    address: Entity<TextInput>,
    key: Entity<TextInput>,
    github_way: Option<GitHubMethod>,
    /// What GitHub's sign-in asks the builder to enter, and where.
    github_code: Option<(String, String)>,
    homebox_outcome: Option<Outcome>,
    github_outcome: Option<Outcome>,
    _submit: Subscription,
    _subscription: Subscription,
}

impl IntegrationsPart {
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
            .with_state(|s| IntegrationsModel::read(&s.daemon, None));
        let address = cx.new(|cx| TextInput::new("https://homebox.example.com", false, cx));
        let (key, submit) = key_field(cx);
        let mut part = Self {
            session,
            model,
            homebox_way: None,
            confirming: false,
            address,
            key,
            github_way: None,
            github_code: None,
            homebox_outcome: None,
            github_outcome: None,
            _submit: submit,
            _subscription: subscription,
        };
        part.look(cx);
        part
    }

    pub fn shown(&self) -> &IntegrationsModel {
        &self.model
    }

    /// The Homebox way the part offers now: the builder's pick, or the daemon's default.
    pub fn homebox_way(&self) -> HomeboxWay {
        self.homebox_way
            .or(self.model.found.as_ref().map(|f| f.homebox.default_way))
            .unwrap_or(HomeboxWay::Address)
    }

    pub fn github_way(&self) -> Option<GitHubMethod> {
        self.github_way
            .or(self.model.found.as_ref().and_then(|f| f.github.default_way))
    }

    pub fn confirming(&self) -> bool {
        self.confirming
    }

    pub fn homebox_outcome(&self) -> Option<&Outcome> {
        self.homebox_outcome.as_ref()
    }

    pub fn github_outcome(&self) -> Option<&Outcome> {
        self.github_outcome.as_ref()
    }

    pub fn github_code(&self) -> Option<&(String, String)> {
        self.github_code.as_ref()
    }

    /// What the key field holds now; empty once the daemon has taken a key.
    pub fn key_text(&self, cx: &gpui::App) -> String {
        self.key.read(cx).text().to_owned()
    }

    fn client(&self, cx: &Context<Self>) -> x100_client::Client {
        self.session.read(cx).client().clone()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let found = self.model.found.clone();
        let now = self
            .session
            .read(cx)
            .client()
            .with_state(|s| IntegrationsModel::read(&s.daemon, found));
        if now != self.model {
            self.model = now;
            cx.notify();
        }
    }

    /// Asks the daemon what it finds on its machine: a local Homebox, a logged-in `gh`.
    pub fn look(&mut self, cx: &mut Context<Self>) {
        let client = self.client(cx);
        notice::run(
            cx,
            async move { client.list_integrations().await },
            |this, result, cx| {
                match result {
                    Ok(found) => this.model.found = Some(found),
                    Err(o) => this.homebox_outcome = Some(o),
                }
                cx.notify();
            },
        );
    }

    pub fn choose_homebox(&mut self, way: HomeboxWay, cx: &mut Context<Self>) {
        self.homebox_way = Some(way);
        self.confirming = false;
        self.homebox_outcome = None;
        cx.notify();
    }

    pub fn choose_github(&mut self, way: GitHubMethod, cx: &mut Context<Self>) {
        self.github_way = Some(way);
        self.github_outcome = None;
        cx.notify();
    }

    /// "Use local": asks first, since the daemon then runs a command in Homebox's container.
    pub fn use_local(&mut self, cx: &mut Context<Self>) {
        self.confirming = true;
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        cx.notify();
    }

    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        let client = self.client(cx);
        self.confirming = false;
        self.homebox_outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.connect_local_homebox().await },
            |this, result, cx| {
                this.homebox_outcome = result.err();
                cx.notify();
            },
        );
    }

    /// Sends the address and the key. Once the daemon has the key the field is replaced with an
    /// empty one, so the key is held nowhere in the app.
    pub fn connect_address(&mut self, cx: &mut Context<Self>) {
        let address = self.address.read(cx).text().trim().to_owned();
        let key = self.key.read(cx).text().trim().to_owned();
        if address.is_empty() || key.is_empty() {
            self.homebox_outcome = Some(Outcome::Failed(
                "Give Homebox's address and an API key made there.".into(),
            ));
            cx.notify();
            return;
        }
        let client = self.client(cx);
        self.homebox_outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.connect_homebox_at(address, key).await },
            |this, result, cx| {
                match result {
                    Ok(_) => {
                        let (key, submit) = key_field(cx);
                        this.key = key;
                        this._submit = submit;
                        this.homebox_outcome = None;
                    }
                    Err(o) => this.homebox_outcome = Some(o),
                }
                cx.notify();
            },
        );
    }

    pub fn connect_github(&mut self, cx: &mut Context<Self>) {
        let Some(way) = self.github_way() else {
            return;
        };
        let client = self.client(cx);
        self.github_outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.connect_github(way).await },
            |this, result, cx| {
                match result {
                    Ok(GitHubConnecting::Approve {
                        user_code,
                        verification_uri,
                    }) => {
                        this.github_code = Some((user_code, verification_uri));
                        this.github_outcome = None;
                    }
                    Ok(GitHubConnecting::Connected { .. }) => this.github_outcome = None,
                    Err(o) => this.github_outcome = Some(o),
                }
                cx.notify();
            },
        );
    }

    pub fn disconnect(&mut self, which: Integration, cx: &mut Context<Self>) {
        let client = self.client(cx);
        let outcome = Some(Outcome::Working);
        match which {
            Integration::Homebox => self.homebox_outcome = outcome,
            Integration::GitHub => {
                self.github_outcome = outcome;
                self.github_code = None;
            }
        }
        cx.notify();
        notice::run(
            cx,
            async move { client.disconnect_integration(which).await },
            move |this, result, cx| {
                match which {
                    Integration::Homebox => this.homebox_outcome = result.err(),
                    Integration::GitHub => this.github_outcome = result.err(),
                }
                // What can be found may have changed since the part last looked.
                this.look(cx);
                cx.notify();
            },
        );
    }

    fn homebox(&self, cx: &mut Context<Self>) -> gpui::Div {
        if let Some(c) = &self.model.homebox {
            let how = match c.way {
                HomeboxWay::Local => "the Homebox on this machine",
                HomeboxWay::Address => "a Homebox elsewhere, with its key",
            };
            let domain = c
                .primary_domain
                .as_ref()
                .map(|d| format!(" Deploys are public under {d}."))
                .unwrap_or_default();
            return status(
                "homebox-status",
                format!(
                    "Connected to {}, {how}. Its key is in x100's vault and used by x100 alone.\
                     {domain}",
                    c.address
                ),
                theme::muted(),
            )
            .child(
                widgets::button("homebox-disconnect", "Disconnect")
                    .debug_selector(|| "homebox-disconnect".into())
                    .on_click(
                        cx.listener(|this, _, _, cx| this.disconnect(Integration::Homebox, cx)),
                    ),
            );
        }
        let local = self
            .model
            .found
            .as_ref()
            .and_then(|f| f.homebox.local.clone());
        let way = self.homebox_way();
        let mut ways = Vec::new();
        if local.is_some() {
            ways.push((HomeboxWay::Local, "Use local", "homebox-way-local"));
        }
        ways.push((HomeboxWay::Address, "Host and key", "homebox-way-address"));
        let choices = ways.into_iter().map(|(w, label, selector)| {
            widgets::segment(selector, label, w == way)
                .debug_selector(move || selector.to_owned())
                .on_click(cx.listener(move |this, _, _, cx| this.choose_homebox(w, cx)))
                .into_any_element()
        });
        let part = status(
            "homebox-status",
            "Not connected. Projects cannot be deployed until it is.".into(),
            theme::ask(),
        )
        .child(widgets::segments(choices));
        match (way, local) {
            (HomeboxWay::Local, Some(at)) if self.confirming => part
                .child(say(format!(
                    "x100 will run one command in Homebox's container, homebox-admin, to make \
                     an edit key named x100 for {at}. The key goes straight to x100's vault."
                )))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            widgets::button("homebox-confirm", "Make the key and connect")
                                .debug_selector(|| "homebox-confirm".into())
                                .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                        )
                        .child(
                            widgets::button("homebox-cancel", "Cancel")
                                .debug_selector(|| "homebox-cancel".into())
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        ),
                ),
            (HomeboxWay::Local, Some(at)) => part
                .child(say(format!("Homebox is running on this machine, at {at}.")))
                .child(
                    widgets::button("homebox-use-local", "Use local")
                        .debug_selector(|| "homebox-use-local".into())
                        .on_click(cx.listener(|this, _, _, cx| this.use_local(cx))),
                ),
            _ => part.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(field("homebox-address", self.address.clone()))
                    .child(field("homebox-key", self.key.clone()))
                    .child(
                        widgets::button("homebox-connect", "Connect")
                            .debug_selector(|| "homebox-connect".into())
                            .on_click(cx.listener(|this, _, _, cx| this.connect_address(cx))),
                    ),
            ),
        }
    }

    fn github(&self, cx: &mut Context<Self>) -> gpui::Div {
        if let Some(g) = &self.model.github {
            let how = match g.way {
                GitHubWay::LocalGh => "through gh on this machine, which keeps its own token",
                GitHubWay::DeviceFlow { .. } => "through GitHub's sign-in",
            };
            return status(
                "github-status",
                format!("Connected as {}, {how}.", g.account),
                theme::muted(),
            )
            .child(
                widgets::button("github-disconnect", "Disconnect")
                    .debug_selector(|| "github-disconnect".into())
                    .on_click(
                        cx.listener(|this, _, _, cx| this.disconnect(Integration::GitHub, cx)),
                    ),
            );
        }
        let ways = self
            .model
            .found
            .as_ref()
            .map(|f| f.github.ways.clone())
            .unwrap_or_default();
        let part = status(
            "github-status",
            "Not connected. A project's code stays on this machine, so Homebox cannot deploy it."
                .into(),
            theme::ask(),
        );
        if ways.is_empty() {
            return part.child(say(
                "Install the gh command line and sign in with `gh auth login`, then reopen \
                 Settings."
                    .into(),
            ));
        }
        let chosen = self.github_way();
        let choices = ways.into_iter().map(|w| {
            let (label, selector) = match w {
                GitHubMethod::LocalGh => ("Use local gh", "github-way-local"),
                GitHubMethod::DeviceFlow => ("Sign in with GitHub", "github-way-signin"),
            };
            widgets::segment(selector, label, Some(w) == chosen)
                .debug_selector(move || selector.to_owned())
                .on_click(cx.listener(move |this, _, _, cx| this.choose_github(w, cx)))
                .into_any_element()
        });
        part.child(widgets::segments(choices))
            .children(self.github_code.as_ref().map(|(code, at)| {
                say(format!("Enter {code} at {at} to let x100 in."))
                    .debug_selector(|| "github-code".into())
            }))
            .child(
                widgets::button("github-connect", "Connect")
                    .debug_selector(|| "github-connect".into())
                    .on_click(cx.listener(|this, _, _, cx| this.connect_github(cx))),
            )
    }
}

fn key_field(cx: &mut Context<IntegrationsPart>) -> (Entity<TextInput>, Subscription) {
    let key = cx.new(|cx| TextInput::new("API key, hbk_…", false, cx).masked());
    let submit = cx.subscribe(&key, |this, _, InputEvent::Submit, cx| {
        this.connect_address(cx)
    });
    (key, submit)
}

/// A service's heading line: what it says about its connection.
fn status(selector: &'static str, said: String, colour: gpui::Rgba) -> gpui::Div {
    div().flex().flex_col().gap(px(6.)).child(
        div()
            .debug_selector(move || selector.to_owned())
            .text_size(px(theme::SMALL))
            .text_color(colour)
            .child(said),
    )
}

fn say(text: String) -> gpui::Div {
    div()
        .text_size(px(theme::SMALL))
        .text_color(theme::muted())
        .child(text)
}

fn field(selector: &'static str, input: Entity<TextInput>) -> gpui::Div {
    div()
        .debug_selector(move || selector.to_owned())
        .flex_1()
        .px(px(10.))
        .py(px(6.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme::line())
        .child(input)
}

fn heading(text: &'static str) -> gpui::Div {
    div()
        .text_size(px(theme::TEXT))
        .font_weight(FontWeight::MEDIUM)
        .child(text)
}

impl Render for IntegrationsPart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .pt(px(14.))
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Integrations"),
            )
            .child(heading("Homebox"))
            .child(self.homebox(cx))
            .children(self.homebox_outcome.as_ref().map(notice::line))
            .child(heading("GitHub"))
            .child(self.github(cx))
            .children(self.github_outcome.as_ref().map(notice::line))
    }
}
