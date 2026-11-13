//! The interface's state: where the builder is, what is open, and how background work lands.
//! Drawing and keys are in their own modules; both act on this.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;

use tokio::sync::mpsc::UnboundedSender;
use x100_client::{Change, Client, ClientError};
use x100_core::file::FileRef;
use x100_core::fold::ProjectPart;
use x100_core::id::{AgentId, CredentialRef, ProjectId};
use x100_core::spec::SpecKind;
use x100_proto::{Memories, Skills};

use crate::input::Input;
use crate::logfeed::{CATCH_UP, LogFeed, PAGE, Read};
use crate::msg::Msg;
use crate::output::Output;
use crate::remember::Remember;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tab {
    Chat,
    Held,
    Team,
    Specs,
    Tasks,
    Files,
    Budget,
    Log,
    Preview,
    Settings,
}

pub const TABS: [Tab; 10] = [
    Tab::Chat,
    Tab::Held,
    Tab::Team,
    Tab::Specs,
    Tab::Tasks,
    Tab::Files,
    Tab::Budget,
    Tab::Log,
    Tab::Preview,
    Tab::Settings,
];

impl Tab {
    pub fn name(self) -> &'static str {
        match self {
            Tab::Chat => "Chat",
            Tab::Held => "Held",
            Tab::Team => "Team",
            Tab::Specs => "Specs",
            Tab::Tasks => "Tasks",
            Tab::Files => "Files",
            Tab::Budget => "Budget",
            Tab::Log => "Log",
            Tab::Preview => "Preview",
            Tab::Settings => "Settings",
        }
    }

    pub fn from_name(name: &str) -> Option<Tab> {
        TABS.into_iter().find(|t| t.name() == name)
    }

    /// Whether a change to `part` is a change to this tab.
    fn shows(self, part: ProjectPart) -> bool {
        matches!(
            (self, part),
            (Tab::Chat, ProjectPart::Chat)
                | (Tab::Held, ProjectPart::HeldActions)
                | (Tab::Team, ProjectPart::Team)
                | (Tab::Specs, ProjectPart::Specs)
                | (Tab::Tasks, ProjectPart::Tasks)
                | (Tab::Files, ProjectPart::Files)
                | (Tab::Budget, ProjectPart::Budget)
                | (Tab::Log, ProjectPart::Log)
                | (Tab::Preview, ProjectPart::Preview)
                | (Tab::Settings, ProjectPart::Settings)
        )
    }
}

/// A question the interface asks in one line before it acts.
pub struct Prompt {
    pub title: String,
    pub input: Input,
    /// Typed characters are shown as dots: a credential's value is entered, never displayed.
    pub masked: bool,
    pub then: Then,
}

/// What a prompt's answer is for.
#[derive(Debug, Clone)]
pub enum Then {
    NewProjectDir,
    AttachPath,
    UploadPath,
    ProjectBudget,
    LabShare,
    LeadBudget(AgentId),
    LeadRange(AgentId),
    RaiseTitle,
    RaiseAsk {
        title: String,
    },
    RemoveFile(x100_core::id::FileId),
    CredentialName,
    CredentialHosts {
        name: String,
    },
    CredentialCommands {
        name: String,
        hosts: String,
    },
    CredentialValue {
        name: String,
        hosts: String,
        commands: String,
    },
    ReplaceValue(CredentialRef),
    RemoveCredential(CredentialRef),
    DecisionProvider,
    DecisionKey(x100_core::decision::DecisionProvider),
    Agent(crate::agent_edits::AgentAsked),
    Integration(crate::integrations::Asked),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Children,
    Output,
    Waits,
    Memories,
    Skills,
    Trials,
}

pub struct AgentUi {
    pub id: AgentId,
    pub mode: AgentMode,
    /// The builder chose what is shown. Until then it is the agents the agent runs, or its output
    /// if it runs none, whichever is so now.
    pub chosen: bool,
    pub output: Output,
    pub memories: Option<Result<Memories, String>>,
    pub skills: Option<Result<Skills, String>>,
}

pub struct ProjectUi {
    pub id: ProjectId,
    pub tab: Tab,
    /// The selected line among those that can be selected, per tab.
    pub sel: HashMap<Tab, usize>,
    /// How far each tab is scrolled: lines from the top, or from the bottom for a tab that
    /// follows its end.
    pub off: HashMap<Tab, usize>,
    /// Tabs that changed while another was open.
    pub dots: BTreeSet<Tab>,
    pub input: Input,
    pub attachments: Vec<FileRef>,
    pub spec: SpecKind,
    pub drivers: bool,
    pub agent: Option<AgentUi>,
    /// The agents opened before this one, so Esc goes back the way the builder came.
    pub back: Vec<AgentId>,
    pub log: LogFeed,
    pub wants_older: bool,
    pub prompt: Option<Prompt>,
    pub agent_sel: usize,
}

impl ProjectUi {
    fn new(id: ProjectId, tab: Tab) -> ProjectUi {
        ProjectUi {
            id,
            tab,
            sel: HashMap::new(),
            off: HashMap::new(),
            dots: BTreeSet::new(),
            input: Input::default(),
            attachments: Vec::new(),
            spec: SpecKind::Brd,
            drivers: false,
            agent: None,
            back: Vec::new(),
            log: LogFeed::default(),
            wants_older: false,
            prompt: None,
            agent_sel: 0,
        }
    }
}

pub enum Where {
    /// Every project, to choose one.
    Projects,
    /// No project yet: a box to write the first message in.
    New,
    Project(Box<ProjectUi>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

pub struct App {
    pub client: Client,
    pub(crate) handle: tokio::runtime::Handle,
    pub(crate) tx: UnboundedSender<Msg>,
    pub at: Where,
    pub list_sel: usize,
    pub new_input: Input,
    pub new_dir: Option<String>,
    pub new_prompt: Option<Prompt>,
    pub notice: Option<Notice>,
    pub quit: bool,
    /// The size of the area the body is drawn in, as last drawn.
    pub body: (u16, u16),
    pub(crate) remember: Remember,
    next_token: u64,
    pub colours: crate::look::Colours,
    pub integrations: crate::integrations::Integrations,
}

impl App {
    pub fn new(
        client: Client,
        handle: tokio::runtime::Handle,
        tx: UnboundedSender<Msg>,
        remember: Remember,
    ) -> App {
        let mut app = App {
            client,
            handle,
            tx,
            at: Where::Projects,
            list_sel: 0,
            new_input: Input::default(),
            new_dir: None,
            new_prompt: None,
            notice: None,
            quit: false,
            body: (100, 30),
            remember,
            next_token: 1,
            colours: crate::look::Colours::from_env(),
            integrations: Default::default(),
        };
        app.at = app.opening();
        if let Where::Project(ui) = &app.at {
            // Opening on the tab does what switching to it does: the Log reads, Settings looks.
            let tab = ui.tab;
            app.set_tab(tab);
        }
        app
    }

    /// Where a run starts: the project and tab last open, else the list, else the first message.
    fn opening(&mut self) -> Where {
        let r = self.remember.read();
        let held = self
            .client
            .with_state(|s| s.projects.keys().copied().collect::<Vec<_>>());
        if let Some(id) = r.project.filter(|p| held.contains(p)) {
            let tab = r
                .tab
                .as_deref()
                .and_then(Tab::from_name)
                .unwrap_or(Tab::Chat);
            return Where::Project(Box::new(ProjectUi::new(id, tab)));
        }
        if held.is_empty() {
            Where::New
        } else {
            Where::Projects
        }
    }

    pub fn open_project(&mut self, id: ProjectId) {
        let ui = ProjectUi::new(id, Tab::Chat);
        self.at = Where::Project(Box::new(ui));
        self.saved();
        self.sync_log_for(id);
    }

    pub fn project_ui(&self) -> Option<&ProjectUi> {
        match &self.at {
            Where::Project(ui) => Some(ui),
            _ => None,
        }
    }

    pub fn project_ui_mut(&mut self) -> Option<&mut ProjectUi> {
        match &mut self.at {
            Where::Project(ui) => Some(ui),
            _ => None,
        }
    }

    /// Writes down where the builder is, for the next run.
    pub fn saved(&self) {
        match &self.at {
            Where::Project(ui) => self.remember.write(Some(ui.id), ui.tab.name()),
            _ => self.remember.write(None, ""),
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        ui.tab = tab;
        ui.dots.remove(&tab);
        let id = ui.id;
        self.saved();
        match tab {
            Tab::Log => self.sync_log_for(id),
            Tab::Settings => {
                // Opened again, Settings reads again, so an answer that failed is asked anew.
                self.integrations.projects = None;
                self.look_around();
                self.sync_homebox_projects();
            }
            _ => {}
        }
    }

    pub fn fresh_token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: false,
        });
    }

    pub fn fail(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            error: true,
        });
    }

    /// Runs `work` in the background and shows its outcome as a line. The line is whatever `Ok`
    /// holds; a refusal from the daemon is shown as it is worded.
    pub fn spawn<F>(&self, work: F)
    where
        F: Future<Output = Result<String, ClientError>> + Send + 'static,
    {
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let msg = match work.await {
                Ok(text) if text.is_empty() => return,
                Ok(text) => Msg::Notice(text),
                Err(e) => Msg::Notice(format!("! {e}")),
            };
            let _ = tx.send(msg);
        });
    }

    /// Applies a message from background work.
    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Notice(text) => match text.strip_prefix("! ") {
                Some(e) => self.fail(e.to_owned()),
                None => self.info(text),
            },
            Msg::Started(Ok(id)) => {
                self.notice = None;
                self.new_input.clear();
                self.new_dir = None;
                self.open_project(id);
            }
            Msg::Started(Err(e)) => self.fail(e.to_string()),
            Msg::Uploaded(Ok(file)) => {
                self.info(format!("Attached {}", file.name));
                if let Some(ui) = self.project_ui_mut() {
                    ui.attachments.push(file);
                }
            }
            Msg::Uploaded(Err(e)) => self.fail(e.to_string()),
            Msg::Log {
                project,
                read,
                result,
            } => self.log_arrived(project, read, result),
            Msg::Memories { agent, result } => {
                if let Some(a) = self.agent_ui_mut(agent) {
                    a.memories = Some(result.map_err(|e| e.to_string()));
                }
            }
            Msg::Skills { agent, result } => {
                if let Some(a) = self.agent_ui_mut(agent) {
                    a.skills = Some(result.map_err(|e| e.to_string()));
                }
            }
            Msg::Entry { token, entry } => {
                let client = self.client.clone();
                if let Some(a) = self.output_mut(token) {
                    client.with_state(|s| {
                        if let Some(p) = s.projects.get(&a.1) {
                            a.0.entry(entry, p, &s.daemon);
                        }
                    });
                }
            }
            Msg::TerminalOpened { token, driver } => {
                if let Some((o, _)) = self.output_mut(token) {
                    o.terminal_opened(driver);
                }
            }
            Msg::Chunk {
                token,
                driver,
                chunk,
            } => {
                if let Some((o, _)) = self.output_mut(token) {
                    o.chunk(driver, chunk);
                }
            }
            Msg::Integration(arrived) => self.integration_arrived(arrived),
            Msg::TerminalRefused {
                token,
                driver,
                no_terminal,
                error,
            } => {
                if let Some((o, _)) = self.output_mut(token) {
                    o.terminal_refused(driver, no_terminal, &error);
                }
            }
        }
    }

    fn agent_ui_mut(&mut self, agent: AgentId) -> Option<&mut AgentUi> {
        self.project_ui_mut()?
            .agent
            .as_mut()
            .filter(|a| a.id == agent)
    }

    fn output_mut(&mut self, token: u64) -> Option<(&mut Output, ProjectId)> {
        let ui = self.project_ui_mut()?;
        let id = ui.id;
        ui.agent
            .as_mut()
            .map(|a| &mut a.output)
            .filter(|o| o.token() == token)
            .map(|o| (o, id))
    }

    /// Takes a change of the client's state: dots on tabs that are not open, the open agent's
    /// output catching up, the log reading what is new.
    pub fn on_change(&mut self, change: &Change) {
        let client = self.client.clone();
        match change {
            Change::Reset => {
                self.sync_homebox_projects();
                if let Some(ui) = self.project_ui_mut() {
                    ui.log.reset();
                    let id = ui.id;
                    self.sync_log_for(id);
                }
            }
            Change::Project { project, changes } => {
                let Some(ui) = self.project_ui_mut() else {
                    return;
                };
                if ui.id != *project {
                    return;
                }
                for tab in TABS {
                    if tab != ui.tab && changes.parts.iter().any(|p| tab.shows(*p)) {
                        ui.dots.insert(tab);
                    }
                }
                let id = ui.id;
                if let Some(a) = &mut ui.agent {
                    client.with_state(|s| {
                        if let Some(p) = s.projects.get(&id) {
                            a.output.pump(p, &s.daemon);
                            if !a.chosen {
                                a.mode = if p.children(a.id).is_empty() {
                                    AgentMode::Output
                                } else {
                                    AgentMode::Children
                                };
                            }
                        }
                    });
                }
                self.sync_log_for(id);
            }
            Change::Daemon(_) => self.sync_homebox_projects(),
            Change::Connection { .. } => {}
        }
    }

    /// Asks for the next page of the log if the Log tab is open and behind.
    pub fn sync_log_for(&mut self, id: ProjectId) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        let handle = self.handle.clone();
        let held = client.project(id).and_then(|p| p.seq());
        let Some(ui) = self.project_ui_mut() else {
            return;
        };
        if ui.id != id || ui.tab != Tab::Log {
            return;
        }
        let wants = ui.wants_older;
        let Some(read) = ui.log.next_read(held, wants) else {
            return;
        };
        ui.wants_older = false;
        handle.spawn(async move {
            let result = match read {
                Read::First => client.read_events_before(id, None, PAGE).await,
                Read::After(n) => client.read_events_after(id, Some(n), CATCH_UP).await,
                Read::Before(o) => client.read_events_before(id, Some(o), PAGE).await,
            };
            let _ = tx.send(Msg::Log {
                project: id,
                read,
                result,
            });
        });
    }

    fn log_arrived(
        &mut self,
        project: ProjectId,
        read: Read,
        result: Result<
            Vec<x100_core::event::Envelope<x100_core::event::ProjectEvent>>,
            ClientError,
        >,
    ) {
        let client = self.client.clone();
        let Some(ui) = self.project_ui_mut().filter(|u| u.id == project) else {
            return;
        };
        match result {
            Ok(events) => client.with_state(|s| {
                if let Some(p) = s.projects.get(&project) {
                    ui.log.ingest(read, events, p, &s.daemon);
                }
            }),
            Err(e) => {
                ui.log.failed();
                self.fail(e.to_string());
                return;
            }
        }
        self.sync_log_for(project);
    }

    /// Starts a project with `message` as its first, on `dir` if one was named.
    pub fn start_project(&mut self, message: String, dir: Option<String>) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        self.handle.spawn(async move {
            let made = async {
                let id = client.create_project(dir).await?;
                client.send_chat(id, message, vec![], None).await?;
                Ok(id)
            }
            .await;
            let _ = tx.send(Msg::Started(made));
        });
    }
}
