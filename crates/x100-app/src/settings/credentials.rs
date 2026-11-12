//! Settings, Credentials: add one with a value, limit the hosts it may reach and the commands that
//! may receive it, choose which projects may use it, replace its value, remove it. A value can be
//! entered and replaced, and is never shown: the state holds names and scopes only.

use std::collections::BTreeSet;

use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use x100_client::Change;
use x100_core::credential::{CredentialScope, Projects};
use x100_core::fold::DaemonPart;
use x100_core::id::{CredentialRef, ProjectId};

use crate::input::{InputEvent, TextInput};
use crate::notice::{self, Outcome};
use crate::session::{Session, SessionEvent};
use crate::widgets;
use crate::{theme, words};

/// A credential as the list shows it. Never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialModel {
    pub reference: CredentialRef,
    pub name: String,
    pub scope: CredentialScope,
}

/// The hosts, commands and projects being chosen, for a new credential or a changed scope.
struct ScopeEditor {
    hosts: Entity<TextInput>,
    commands: Entity<TextInput>,
    all: bool,
    projects: BTreeSet<ProjectId>,
}

impl ScopeEditor {
    fn new(
        from: Option<&CredentialScope>,
        this_project: ProjectId,
        cx: &mut Context<Credentials>,
    ) -> Self {
        let hosts =
            cx.new(|cx| TextInput::new("Hosts it may reach, separated by commas", false, cx));
        let commands = cx.new(|cx| {
            TextInput::new(
                "Commands that may receive it, separated by commas",
                false,
                cx,
            )
        });
        let (all, projects) = match from.map(|s| &s.projects) {
            Some(Projects::All) => (true, BTreeSet::new()),
            Some(Projects::Only(p)) => (false, p.clone()),
            None => (false, BTreeSet::from([this_project])),
        };
        if let Some(scope) = from {
            hosts.update(cx, |i, cx| {
                i.set_text(
                    scope.hosts.iter().cloned().collect::<Vec<_>>().join(", "),
                    cx,
                )
            });
            commands.update(cx, |i, cx| {
                i.set_text(
                    scope
                        .commands
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    cx,
                )
            });
        }
        Self {
            hosts,
            commands,
            all,
            projects,
        }
    }

    fn scope(&self, cx: &Context<Credentials>) -> CredentialScope {
        let list = |input: &Entity<TextInput>| -> BTreeSet<String> {
            input
                .read(cx)
                .text()
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        };
        CredentialScope {
            hosts: list(&self.hosts),
            commands: list(&self.commands),
            projects: if self.all {
                Projects::All
            } else {
                Projects::Only(self.projects.clone())
            },
        }
    }
}

enum Edit {
    Replace(Entity<TextInput>),
    Scope(ScopeEditor),
    Remove,
}

struct Adding {
    name: Entity<TextInput>,
    value: Entity<TextInput>,
    scope: ScopeEditor,
    _submit: Subscription,
}

pub struct Credentials {
    session: Entity<Session>,
    project: ProjectId,
    list: Vec<CredentialModel>,
    adding: Option<Adding>,
    editing: Option<(CredentialRef, Edit)>,
    outcome: Option<Outcome>,
    _subscription: Subscription,
}

impl Credentials {
    pub fn new(session: Entity<Session>, project: ProjectId, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event, cx| {
            let ours = match event {
                SessionEvent::Changed(Change::Daemon(c)) => {
                    c.parts.contains(&DaemonPart::Credentials)
                        || c.parts.contains(&DaemonPart::Projects)
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
            list: Vec::new(),
            adding: None,
            editing: None,
            outcome: None,
            _subscription: subscription,
        };
        view.refresh(cx);
        view
    }

    pub fn shown(&self) -> &[CredentialModel] {
        &self.list
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now: Vec<CredentialModel> = self.session.read(cx).client().with_state(|s| {
            s.daemon
                .credentials()
                .map(|c| CredentialModel {
                    reference: c.credential.reference,
                    name: c.credential.name.clone(),
                    scope: c.credential.scope.clone(),
                })
                .collect()
        });
        if now != self.list {
            self.list = now;
            cx.notify();
        }
    }

    fn start_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| TextInput::new("Name, such as Stripe test key", false, cx));
        let value = cx.new(|cx| TextInput::new("Value", false, cx).masked());
        let submit = cx.subscribe(&value, |this, _, InputEvent::Submit, cx| this.add(cx));
        window.focus(&gpui::Focusable::focus_handle(&name, cx), cx);
        let scope = ScopeEditor::new(None, self.project, cx);
        self.adding = Some(Adding {
            name,
            value,
            scope,
            _submit: submit,
        });
        self.outcome = None;
        cx.notify();
    }

    pub fn add(&mut self, cx: &mut Context<Self>) {
        let Some(a) = &self.adding else { return };
        let name = a.name.read(cx).text().trim().to_owned();
        let value = a.value.read(cx).text().to_owned();
        if name.is_empty() || value.is_empty() {
            self.outcome = Some(Outcome::Failed(
                "A credential needs a name and a value.".into(),
            ));
            cx.notify();
            return;
        }
        let scope = a.scope.scope(cx);
        let client = self.session.read(cx).client().clone();
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(
            cx,
            async move { client.add_credential(name, value, scope).await },
            |this, result, cx| {
                match result {
                    Ok(_) => {
                        this.adding = None;
                        this.outcome = None;
                    }
                    Err(o) => this.outcome = Some(o),
                }
                cx.notify();
            },
        );
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some((reference, edit)) = &self.editing else {
            return;
        };
        let reference = *reference;
        let client = self.session.read(cx).client().clone();
        let request = match edit {
            Edit::Replace(value) => {
                let value = value.read(cx).text().to_owned();
                if value.is_empty() {
                    self.outcome = Some(Outcome::Failed("Enter the new value.".into()));
                    cx.notify();
                    return;
                }
                let client = client.clone();
                futures_box(async move { client.replace_credential(reference, value).await })
            }
            Edit::Scope(editor) => {
                let scope = editor.scope(cx);
                futures_box(async move { client.scope_credential(reference, scope).await })
            }
            Edit::Remove => futures_box(async move { client.remove_credential(reference).await }),
        };
        self.outcome = Some(Outcome::Working);
        cx.notify();
        notice::run(cx, request, |this, result, cx| {
            match result {
                Ok(()) => {
                    this.editing = None;
                    this.outcome = None;
                }
                Err(o) => this.outcome = Some(o),
            }
            cx.notify();
        });
    }

    fn edit(
        &mut self,
        reference: CredentialRef,
        kind: EditKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = match kind {
            EditKind::Replace => {
                let value = cx.new(|cx| TextInput::new("New value", false, cx).masked());
                window.focus(&gpui::Focusable::focus_handle(&value, cx), cx);
                Edit::Replace(value)
            }
            EditKind::Scope => {
                let scope = self
                    .list
                    .iter()
                    .find(|c| c.reference == reference)
                    .map(|c| c.scope.clone());
                Edit::Scope(ScopeEditor::new(scope.as_ref(), self.project, cx))
            }
            EditKind::Remove => Edit::Remove,
        };
        self.editing = Some((reference, edit));
        self.outcome = None;
        cx.notify();
    }

    fn scope_controls(
        &self,
        editor: &ScopeEditor,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let projects: Vec<(ProjectId, String)> = self.session.read(cx).client().with_state(|s| {
            s.daemon
                .projects()
                .map(|(id, _)| (id, words::project_name(s.projects.get(&id))))
                .collect()
        });
        let field = |input: &Entity<TextInput>| {
            div()
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme::line())
                .child(input.clone())
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(field(&editor.hosts))
            .child(field(&editor.commands))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(theme::SMALL))
                            .text_color(theme::muted())
                            .child("Projects that may use it"),
                    )
                    .child(
                        widgets::segment((key, 0usize), "All projects", editor.all)
                            .debug_selector(move || format!("{key}-all"))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_all(key, cx))),
                    )
                    .children(projects.into_iter().enumerate().map(|(i, (id, name))| {
                        let on = !editor.all && editor.projects.contains(&id);
                        let selector = format!("{key}-project-{name}");
                        widgets::segment((key, i + 1), name, on)
                            .debug_selector(move || selector.clone())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_project(key, id, cx)),
                            )
                    })),
            )
            .into_any_element()
    }

    fn editor_mut(&mut self, key: &'static str) -> Option<&mut ScopeEditor> {
        match key {
            "cred-add" => self.adding.as_mut().map(|a| &mut a.scope),
            _ => match &mut self.editing {
                Some((_, Edit::Scope(e))) => Some(e),
                _ => None,
            },
        }
    }

    fn toggle_all(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if let Some(e) = self.editor_mut(key) {
            e.all = !e.all;
            cx.notify();
        }
    }

    fn toggle_project(&mut self, key: &'static str, project: ProjectId, cx: &mut Context<Self>) {
        if let Some(e) = self.editor_mut(key) {
            e.all = false;
            if !e.projects.remove(&project) {
                e.projects.insert(project);
            }
            cx.notify();
        }
    }
}

#[derive(Clone, Copy)]
enum EditKind {
    Replace,
    Scope,
    Remove,
}

/// A request future of unit result, boxed so the arms of a match have one type.
fn futures_box(
    f: impl std::future::Future<Output = Result<(), x100_client::ClientError>> + 'static,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), x100_client::ClientError>>>> {
    Box::pin(f)
}

fn scope_words(scope: &CredentialScope, projects: &[(ProjectId, String)]) -> String {
    let list = |s: &BTreeSet<String>| {
        if s.is_empty() {
            "none".to_owned()
        } else {
            s.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    };
    let who = match &scope.projects {
        Projects::All => "all projects".to_owned(),
        Projects::Only(set) => {
            let names: Vec<_> = set
                .iter()
                .map(|id| {
                    projects
                        .iter()
                        .find(|(p, _)| p == id)
                        .map_or("another project", |(_, n)| n.as_str())
                })
                .collect();
            if names.is_empty() {
                "no project".to_owned()
            } else {
                names.join(", ")
            }
        }
    };
    format!(
        "Hosts: {} \u{b7} Commands: {} \u{b7} Projects: {who}",
        list(&scope.hosts),
        list(&scope.commands)
    )
}

impl Render for Credentials {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let projects: Vec<(ProjectId, String)> = self.session.read(cx).client().with_state(|s| {
            s.daemon
                .projects()
                .map(|(id, _)| (id, words::project_name(s.projects.get(&id))))
                .collect()
        });
        let field = |sel: &'static str, input: &Entity<TextInput>| {
            div()
                .debug_selector(move || sel.into())
                .px(px(10.))
                .py(px(6.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme::line())
                .child(input.clone())
        };
        let rows: Vec<_> = self
            .list
            .clone()
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                let reference = c.reference;
                let name = c.name.clone();
                let editing = self.editing.as_ref().filter(|(r, _)| *r == reference);
                let button = |id: &'static str, label: &'static str, kind: EditKind, cx: &mut Context<Self>| {
                    let name = name.clone();
                    widgets::button((id, i), label)
                        .debug_selector(move || format!("{id}-{name}"))
                        .on_click(cx.listener(move |this, _, window, cx| this.edit(reference, kind, window, cx)))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(theme::line())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(theme::TEXT)).font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                                    .child(div().text_size(px(theme::TINY)).text_color(theme::muted()).child(scope_words(&c.scope, &projects))),
                            )
                            .child(button("cred-replace", "Replace value", EditKind::Replace, cx))
                            .child(button("cred-scope", "Limit", EditKind::Scope, cx))
                            .child(button("cred-remove", "Remove", EditKind::Remove, cx)),
                    )
                    .when_some(editing.map(|(_, e)| e), |d, edit| match edit {
                        Edit::Replace(value) => d.child(field("cred-new-value", value)),
                        Edit::Scope(editor) => d.child(self.scope_controls(editor, "cred-edit", cx)),
                        Edit::Remove => d.child(
                            div()
                                .text_size(px(theme::SMALL))
                                .text_color(theme::error())
                                .child("Remove this credential and its value? Agents holding a reference to it lose access."),
                        ),
                    })
                    .when(editing.is_some(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    widgets::button(("cred-save", i), "Confirm")
                                        .debug_selector(|| "cred-save".into())
                                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                                )
                                .child(
                                    widgets::button(("cred-cancel", i), "Cancel").on_click(cx.listener(|this, _, _, cx| {
                                        this.editing = None;
                                        this.outcome = None;
                                        cx.notify();
                                    })),
                                )
                                .children(self.outcome.as_ref().map(notice::line)),
                        )
                    })
            })
            .collect();
        let add = match &self.adding {
            None => div()
                .child(
                    widgets::button("cred-add-open", "Add a credential")
                        .debug_selector(|| "cred-add-open".into())
                        .on_click(cx.listener(|this, _, window, cx| this.start_adding(window, cx))),
                )
                .into_any_element(),
            Some(a) => {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(gpui::rgba(0xffffff60))
                    .child(field("cred-name", &a.name))
                    .child(field("cred-value", &a.value))
                    .child(self.scope_controls(&a.scope, "cred-add", cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                widgets::button("cred-add-send", "Add")
                                    .debug_selector(|| "cred-add-send".into())
                                    .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                            )
                            .child(widgets::button("cred-add-cancel", "Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.adding = None;
                                    this.outcome = None;
                                    cx.notify();
                                }),
                            ))
                            .children(
                                self.outcome
                                    .as_ref()
                                    .filter(|_| self.editing.is_none())
                                    .map(notice::line),
                            ),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(theme::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Credentials"),
            )
            .child(
                div()
                    .text_size(px(theme::TINY))
                    .text_color(theme::dim())
                    .child("Agents hold a reference to a credential, never its value. A value is entered here and can be replaced, and is never shown."),
            )
            .children(rows)
            .when(self.list.is_empty(), |d| {
                d.child(div().text_size(px(theme::SMALL)).text_color(theme::dim()).child("No credentials yet."))
            })
            .child(add)
    }
}
