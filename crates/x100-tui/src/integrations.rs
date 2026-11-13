//! Settings, Integrations and the project's Homebox project, as the desktop app has them: Homebox,
//! where projects are deployed, connected through the one on this machine or an address and a key;
//! GitHub through the local `gh`, or GitHub's sign-in where the daemon has an app for it; the
//! project's link to one of Homebox's projects; and a deploy. A key is typed masked and goes to
//! the daemon once; nothing here keeps it.

use x100_client::ClientError;
use x100_core::id::ProjectId;
use x100_proto::{GitHubConnecting, GitHubMethod, HomeboxProject, Integration, IntegrationList};

use crate::app::{App, Then};
use crate::msg::Msg;
use crate::page::Setting;

/// What the part shows beyond the client's state, which has the connections themselves.
#[derive(Debug, Default)]
pub struct Integrations {
    /// What the daemon found on its machine: a local Homebox, a logged-in `gh`. `None` until it
    /// has said.
    pub found: Option<Result<IntegrationList, String>>,
    /// The connected Homebox's projects.
    pub projects: Option<Listed>,
    /// What GitHub's sign-in asks the builder to enter, and where.
    pub github_code: Option<(String, String)>,
}

/// A Homebox's projects, keyed by the address they were asked of, so an answer about a Homebox no
/// longer connected is dropped.
#[derive(Debug)]
pub struct Listed {
    pub address: String,
    /// `None` while it is being asked.
    pub projects: Option<Result<Vec<HomeboxProject>, String>>,
}

/// A prompt's purpose in this part.
#[derive(Debug, Clone)]
pub enum Asked {
    /// "Use local" asks first, since the daemon then runs a command in Homebox's container.
    UseLocal,
    Address,
    /// The key for the Homebox at `address`, typed masked.
    Key {
        address: String,
    },
    Disconnect(Integration),
}

pub enum Arrived {
    Found(Result<IntegrationList, ClientError>),
    Projects {
        address: String,
        result: Result<Vec<HomeboxProject>, ClientError>,
    },
    GitHubCode {
        code: String,
        at: String,
    },
}

impl App {
    /// Asks the daemon what it finds on its machine, as Settings does each time it is opened.
    pub(crate) fn look_around(&mut self) {
        let (client, tx) = (self.client.clone(), self.tx.clone());
        self.handle.spawn(async move {
            let found = client.list_integrations().await;
            let _ = tx.send(Msg::Integration(Arrived::Found(found)));
        });
    }

    /// Asks the connected Homebox for its projects when it is not the one they were read from.
    pub(crate) fn sync_homebox_projects(&mut self) {
        let connected = self.client.with_state(|s| {
            s.daemon
                .settings()
                .homebox
                .as_ref()
                .map(|c| c.address.clone())
        });
        let asked = self.integrations.projects.as_ref().map(|l| &l.address);
        if connected.as_ref() == asked {
            return;
        }
        self.integrations.projects = connected.clone().map(|address| Listed {
            address,
            projects: None,
        });
        let Some(address) = connected else {
            return;
        };
        let (client, tx) = (self.client.clone(), self.tx.clone());
        self.handle.spawn(async move {
            let result = client.homebox_projects().await.map(|p| p.projects);
            let _ = tx.send(Msg::Integration(Arrived::Projects { address, result }));
        });
    }

    pub(crate) fn integration_arrived(&mut self, arrived: Arrived) {
        let ig = &mut self.integrations;
        match arrived {
            Arrived::Found(found) => ig.found = Some(found.map_err(|e| e.to_string())),
            Arrived::Projects { address, result } => {
                if let Some(listed) = &mut ig.projects
                    && listed.address == address
                {
                    listed.projects = Some(result.map_err(|e| e.to_string()));
                }
            }
            Arrived::GitHubCode { code, at } => ig.github_code = Some((code, at)),
        }
    }

    /// Enter on one of this part's lines.
    pub(crate) fn integration_chosen(&mut self, project: ProjectId, s: Setting) {
        let client = self.client.clone();
        match s {
            Setting::HomeboxLocal => {
                let at = self
                    .integrations
                    .found
                    .as_ref()
                    .and_then(|f| f.as_ref().ok())
                    .and_then(|f| f.homebox.local.clone())
                    .unwrap_or_default();
                self.ask(
                    &format!(
                        "x100 will run one command in Homebox's container, homebox-admin, to make \
                         an edit key named x100 for {at}. The key goes straight to x100's vault. \
                         Go ahead? (y/n)"
                    ),
                    Then::Integration(Asked::UseLocal),
                );
            }
            Setting::HomeboxAddress => self.ask(
                "Homebox's address, such as https://homebox.example.com",
                Then::Integration(Asked::Address),
            ),
            Setting::Disconnect(which) => self.ask(
                &format!("Disconnect {}? (y/n)", name(which)),
                Then::Integration(Asked::Disconnect(which)),
            ),
            Setting::GitHub(way) => {
                self.integrations.github_code = None;
                let tx = self.tx.clone();
                self.spawn(async move {
                    match client.connect_github(way).await? {
                        GitHubConnecting::Connected { account } => {
                            Ok(format!("Connected GitHub as {account}"))
                        }
                        GitHubConnecting::Approve {
                            user_code,
                            verification_uri,
                        } => {
                            let _ = tx.send(Msg::Integration(Arrived::GitHubCode {
                                code: user_code,
                                at: verification_uri,
                            }));
                            Ok(String::new())
                        }
                    }
                });
            }
            Setting::LinkHomebox(repo) => self.spawn(async move {
                let link = client.link_homebox(project, repo, None).await?;
                Ok(format!(
                    "Linked to {}, which deploys to {}",
                    link.project_name, link.environment
                ))
            }),
            Setting::UnlinkHomebox => self.spawn(async move {
                client.unlink_homebox(project).await?;
                Ok("Unlinked from Homebox".into())
            }),
            Setting::Deploy => {
                self.info("Deploying…");
                self.spawn(async move {
                    let done = client.deploy(project, None).await?;
                    Ok(match done.address {
                        Some(at) => format!("Deployed {} at {at}", short(&done.commit)),
                        None => format!("Deployed {}", short(&done.commit)),
                    })
                });
            }
            _ => {}
        }
    }

    pub(crate) fn integration_answered(&mut self, asked: Asked, text: String, yes: bool) {
        let client = self.client.clone();
        match asked {
            Asked::UseLocal if yes => self.spawn(async move {
                let c = client.connect_local_homebox().await?;
                Ok(format!("Connected Homebox at {}", c.connection.address))
            }),
            Asked::UseLocal => {}
            Asked::Address if text.is_empty() => {}
            Asked::Address => self.ask_secret(
                "API key made in Homebox (it is sent, never shown)",
                Then::Integration(Asked::Key { address: text }),
            ),
            Asked::Key { .. } if text.is_empty() => {
                self.fail("Give Homebox's address and an API key made there.")
            }
            // The key leaves here; the only copy kept is the one in this task.
            Asked::Key { address } => self.spawn(async move {
                let c = client.connect_homebox_at(address, text).await?;
                Ok(format!("Connected Homebox at {}", c.connection.address))
            }),
            Asked::Disconnect(which) if yes => {
                if which == Integration::GitHub {
                    self.integrations.github_code = None;
                }
                let tx = self.tx.clone();
                self.spawn(async move {
                    client.disconnect_integration(which).await?;
                    // What can be found may have changed since Settings last looked.
                    let found = client.list_integrations().await;
                    let _ = tx.send(Msg::Integration(Arrived::Found(found)));
                    Ok(format!("Disconnected {}", name(which)))
                });
            }
            Asked::Disconnect(_) => {}
        }
    }
}

pub fn name(which: Integration) -> &'static str {
    match which {
        Integration::Homebox => "Homebox",
        Integration::GitHub => "GitHub",
    }
}

pub fn method(way: GitHubMethod) -> &'static str {
    match way {
        GitHubMethod::LocalGh => "Use local gh",
        GitHubMethod::DeviceFlow => "Sign in with GitHub",
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}
