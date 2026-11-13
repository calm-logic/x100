//! The keys and answers of the Settings tab, and the prompts' answers every tab shares.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use x100_core::action::AskMeAbout;
use x100_core::money::Percent;
use x100_core::project::SettingChange;

use crate::app::{App, Then};
use crate::msg::Msg;
use crate::page::{Setting, Target};

impl App {
    pub(crate) fn key_settings(&mut self, key: KeyEvent) {
        let Some(project) = self.project_ui().map(|u| u.id) else {
            return;
        };
        let target = self.selected();
        let client = self.client.clone();
        match (key.code, target) {
            (KeyCode::Char('c'), _) => self.ask("Name of the credential", Then::CredentialName),
            (KeyCode::Char('v'), Some(Target::Credential(c))) => {
                self.ask_secret("New value (it is sent, never shown)", Then::ReplaceValue(c))
            }
            (KeyCode::Char('x'), Some(Target::Credential(c))) => {
                self.ask("Remove this credential? (y/n)", Then::RemoveCredential(c))
            }
            (KeyCode::Char('r'), Some(Target::Lead(l))) => self.ask(
                "Effort range, as `low high` (low, medium, high, max)",
                Then::LeadRange(l),
            ),
            (KeyCode::Enter, Some(Target::Setting(s))) => {
                let p = client.project(project);
                let Some(settings) = p
                    .as_ref()
                    .and_then(|p| p.project())
                    .map(|p| p.settings.clone())
                else {
                    return;
                };
                match s {
                    Setting::AskMeAbout => {
                        let next = match settings.ask_me_about {
                            AskMeAbout::Nothing => AskMeAbout::HighRiskOnly,
                            AskMeAbout::HighRiskOnly => AskMeAbout::MediumAndUp,
                            AskMeAbout::MediumAndUp => AskMeAbout::EveryDecision,
                            AskMeAbout::EveryDecision => AskMeAbout::Nothing,
                        };
                        self.change(project, SettingChange::AskMeAbout(next));
                    }
                    Setting::Budget => self.ask("Project budget in dollars", Then::ProjectBudget),
                    Setting::LabShare => {
                        self.ask("Lab share of the budget, 0 to 100", Then::LabShare)
                    }
                    Setting::TrialBeforeCommit => self.change(
                        project,
                        SettingChange::TrialBeforeCommit(!settings.trial_before_commit),
                    ),
                    Setting::Allow(what) => {
                        let allowed = !settings.allowed.contains(&what);
                        self.change(project, SettingChange::Allow { what, allowed });
                    }
                    Setting::DecisionModel => {
                        if self
                            .client
                            .with_state(|s| s.daemon.settings().decision_model.is_some())
                        {
                            self.spawn(async move {
                                client.disconnect_decision_model().await?;
                                Ok("Disconnected the decision model".into())
                            });
                        } else {
                            self.ask(
                                "Provider: 1 Gemini, 2 OpenAI, 3 Jev",
                                Then::DecisionProvider,
                            );
                        }
                    }
                    Setting::ReflectNow => self.spawn(async move {
                        client.reflect(project).await?;
                        Ok("The CEO was asked to reflect".into())
                    }),
                }
            }
            _ => {}
        }
    }

    pub(crate) fn change(&mut self, project: x100_core::id::ProjectId, change: SettingChange) {
        let client = self.client.clone();
        self.spawn(async move {
            client.change_setting(project, change).await?;
            Ok(String::new())
        });
    }

    /// Acts on what a prompt was for.
    pub(crate) fn answered(&mut self, then: Then, text: String) {
        use x100_client::words::parse_money;
        let text = text.trim().to_owned();
        let Some(project) = self.project_ui().map(|u| u.id) else {
            return;
        };
        let client = self.client.clone();
        let yes = text.eq_ignore_ascii_case("y") || text.eq_ignore_ascii_case("yes");
        match then {
            Then::NewProjectDir => {}
            Then::AttachPath | Then::UploadPath => {
                self.upload(project, &text, matches!(then, Then::AttachPath))
            }
            Then::ProjectBudget => match parse_money(&text) {
                Some(m) => self.change(project, SettingChange::Budget(m)),
                None => self.fail("That is not an amount of dollars"),
            },
            Then::LabShare => match text.parse::<u8>().ok().and_then(|n| Percent::new(n).ok()) {
                Some(p) => self.change(project, SettingChange::LabShare(p)),
                None => self.fail("That is not a share from 0 to 100"),
            },
            Then::LeadBudget(lead) => match parse_money(&text) {
                Some(m) => self.spawn(async move {
                    client.set_lead_budget(project, lead, m).await?;
                    Ok(String::new())
                }),
                None => self.fail("That is not an amount of dollars"),
            },
            Then::LeadRange(lead) => match parse_range(&text) {
                Some(r) => self.spawn(async move {
                    client.set_effort_range(project, lead, r).await?;
                    Ok(String::new())
                }),
                None => self.fail("Give two efforts, lowest first: low, medium, high or max"),
            },
            Then::RaiseTitle if !text.is_empty() => {
                self.ask("What is asked", Then::RaiseAsk { title: text })
            }
            Then::RaiseTitle => {}
            Then::RaiseAsk { title } => self.spawn(async move {
                client.raise_task(project, title, text, None).await?;
                Ok("Raised the task".into())
            }),
            Then::RemoveFile(file) if yes => self.spawn(async move {
                client.remove_file(project, file).await?;
                Ok("Removed the file".into())
            }),
            Then::RemoveFile(_) => {}
            Then::CredentialName if !text.is_empty() => self.ask(
                "Hosts it may reach, separated by commas",
                Then::CredentialHosts { name: text },
            ),
            Then::CredentialName => {}
            Then::CredentialHosts { name } => self.ask(
                "Commands that may receive it, separated by commas",
                Then::CredentialCommands { name, hosts: text },
            ),
            Then::CredentialCommands { name, hosts } => self.ask_secret(
                "Value (it is sent, never shown)",
                Then::CredentialValue {
                    name,
                    hosts,
                    commands: text,
                },
            ),
            Then::CredentialValue {
                name,
                hosts,
                commands,
            } => {
                let list = |s: &str| -> std::collections::BTreeSet<String> {
                    s.split(',')
                        .map(str::trim)
                        .filter(|x| !x.is_empty())
                        .map(str::to_owned)
                        .collect()
                };
                let scope = x100_core::credential::CredentialScope {
                    hosts: list(&hosts),
                    commands: list(&commands),
                    projects: x100_core::credential::Projects::Only([project].into()),
                };
                // The value leaves here; the only copy kept is the one in this task.
                self.spawn(async move {
                    client.add_credential(name, text, scope).await?;
                    Ok("Added the credential".into())
                });
            }
            Then::ReplaceValue(c) => self.spawn(async move {
                client.replace_credential(c, text).await?;
                Ok("Replaced the value".into())
            }),
            Then::RemoveCredential(c) => {
                if yes {
                    self.spawn(async move {
                        client.remove_credential(c).await?;
                        Ok("Removed the credential".into())
                    });
                }
            }
            Then::DecisionProvider => {
                use x100_core::decision::DecisionProvider as P;
                let provider = match text.to_ascii_lowercase().as_str() {
                    "1" | "gemini" => Some(P::Gemini),
                    "2" | "openai" => Some(P::OpenAi),
                    "3" | "jev" => Some(P::Jev),
                    _ => None,
                };
                match provider {
                    Some(p) => {
                        self.ask_secret("API key (it is sent, never shown)", Then::DecisionKey(p))
                    }
                    None => self.fail("Choose 1, 2 or 3"),
                }
            }
            Then::DecisionKey(provider) => self.spawn(async move {
                client.connect_decision_model(provider, text).await?;
                Ok("Connected the decision model".into())
            }),
        }
    }

    fn upload(&mut self, project: x100_core::id::ProjectId, path: &str, attach: bool) {
        let path = std::path::PathBuf::from(path);
        let (client, tx) = (self.client.clone(), self.tx.clone());
        self.handle.spawn(async move {
            let name = path
                .file_name()
                .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
            let msg = match tokio::fs::read(&path).await {
                Err(e) => Msg::Notice(format!("! {}: {e}", path.display())),
                Ok(data) => {
                    let media = x100_client::words::media_type(&name);
                    match client.upload_file(project, name, media, data).await {
                        Ok(file) if attach => Msg::Uploaded(Ok(file)),
                        Ok(file) => Msg::Notice(format!("Added {}", file.name)),
                        Err(e) => Msg::Notice(format!("! {e}")),
                    }
                }
            };
            let _ = tx.send(msg);
        });
    }
}

fn parse_range(text: &str) -> Option<x100_core::effort::EffortRange> {
    use x100_core::effort::{Effort, EffortRange};
    let one = |s: &str| match s.to_ascii_lowercase().as_str() {
        "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "max" => Some(Effort::Max),
        _ => None,
    };
    let mut parts = text.split_whitespace();
    let (min, max) = (one(parts.next()?)?, one(parts.next()?)?);
    EffortRange::new(min, max).ok()
}
