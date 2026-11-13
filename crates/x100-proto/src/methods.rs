//! The method table. One entry per method gives its name, params type, result type and the errors
//! it can return, and the doc comment is the method's summary in `PROTOCOL.md`. The macro makes
//! [`Call`], the typed request, so a method cannot exist without all four.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{ErrorCode, RpcError};
use crate::types::*;
use crate::{EventNotice, SubscriptionClosed, TerminalNotice, TranscriptNotice};

/// Implemented by each params type: the method it is for and what comes back.
pub trait Method: Serialize + DeserializeOwned {
    const NAME: &'static str;
    type Result: Serialize + DeserializeOwned;
    const ERRORS: &'static [ErrorCode];
}

#[derive(Debug, Clone, Copy)]
pub struct MethodInfo {
    pub name: &'static str,
    pub summary: &'static str,
    /// The Rust types of the params and the result, as written in the table.
    pub params: &'static str,
    pub result: &'static str,
    pub errors: &'static [ErrorCode],
}

macro_rules! methods {
    ($(
        $(#[doc = $doc:literal])+
        $variant:ident, $name:literal, $params:ty => $result:ty, [$($err:ident),*];
    )*) => {
        /// A request: the method and its params.
        #[derive(Debug, Clone, PartialEq)]
        pub enum Call {
            $($variant($params),)*
        }

        $(
            impl Method for $params {
                const NAME: &'static str = $name;
                type Result = $result;
                const ERRORS: &'static [ErrorCode] = &[$(ErrorCode::$err),*];
            }

            impl From<$params> for Call {
                fn from(p: $params) -> Call {
                    Call::$variant(p)
                }
            }
        )*

        /// Every method, in the order `PROTOCOL.md` lists them.
        pub const METHODS: &[MethodInfo] = &[$(MethodInfo {
            name: $name,
            summary: concat!($($doc, "\n"),+),
            params: stringify!($params),
            result: stringify!($result),
            errors: &[$(ErrorCode::$err),*],
        }),*];

        impl Call {
            pub fn method(&self) -> &'static str {
                match self {
                    $(Call::$variant(_) => $name,)*
                }
            }

            pub fn params(&self) -> Value {
                match self {
                    $(Call::$variant(p) => serde_json::to_value(p).expect("params serialise"),)*
                }
            }

            /// Reads params for `method`: `MethodNotFound` or `InvalidParams`.
            pub fn parse(method: &str, params: Value) -> Result<Call, RpcError> {
                match method {
                    $($name => serde_json::from_value(params)
                        .map(Call::$variant)
                        .map_err(|e| RpcError::new(ErrorCode::InvalidParams, e.to_string())),)*
                    other => Err(RpcError::new(
                        ErrorCode::MethodNotFound,
                        format!("no method `{other}`"),
                    )),
                }
            }
        }

        /// Reads `value` as the result of `method` and writes it out again, so a caller can check
        /// that nothing in it was unknown or lost.
        pub fn reserialize_result(method: &str, value: Value) -> Result<Value, RpcError> {
            match method {
                $($name => serde_json::from_value::<$result>(value)
                    .map(|r| serde_json::to_value(r).expect("results serialise"))
                    .map_err(|e| RpcError::new(ErrorCode::InvalidParams, e.to_string())),)*
                other => Err(RpcError::new(
                    ErrorCode::MethodNotFound,
                    format!("no method `{other}`"),
                )),
            }
        }
    };
}

methods! {
    /// The first request on a connection. `protocol` is the highest version the client speaks; the
    /// reply carries the version both sides use.
    Hello, "hello", Hello => Welcome, [UnsupportedProtocol];

    /// Subscribes to the daemon's log and every project's. Without `resume` the result carries a
    /// snapshot, and every event notified on the subscription is after it, once, in order, per log.
    /// With `resume`, the events after the cursor are notified first and the result carries no
    /// snapshot. A project created later is followed from its first event.
    Subscribe, "events.subscribe", Subscribe => Subscribed, [Rejected];

    /// Ends a subscription, of events, of a transcript or of a terminal.
    Unsubscribe, "unsubscribe", Unsubscribe => Done, [UnknownSubscription];

    /// Reads a page of the daemon's log or a project's: newest first for the Log tab, or oldest
    /// first from a point.
    ReadLog, "events.read", ReadLog => LogPage, [NotFound];

    /// Reads a page of an agent's transcript, after a position.
    ReadTranscript, "transcript.read", ReadTranscript => TranscriptPage, [NotFound];

    /// Follows an agent's transcript: the entries after `after` are notified first, then new ones as
    /// they are written, with none missed or repeated.
    SubscribeTranscript, "transcript.subscribe", SubscribeTranscript => TranscriptSubscribed, [NotFound];

    /// Reads a page of a terminal driver's terminal, after a position.
    ReadTerminal, "terminal.read", ReadTerminal => TerminalPage, [NotFound];

    /// Follows a terminal driver's terminal: the chunks after `after` are notified first, then new
    /// ones as they are written, with none missed or repeated. A driver with no terminal is
    /// `not_found`.
    SubscribeTerminal, "terminal.subscribe", SubscribeTerminal => TerminalSubscribed, [NotFound];

    /// Sets a live terminal driver's terminal to `cols` by `rows`, the cells a viewer's pane
    /// holds, and its program redraws itself at that size. The resize is recorded among the
    /// driver's chunks where the program's output starts to follow it, and a size the terminal
    /// has already records nothing. It is the only thing a viewer sends a terminal. A driver with
    /// no terminal is `not_found`, one that has ended `wrong_state`, and a size outside 20 by 5 to
    /// 1000 by 500 `rejected`.
    ResizeTerminal, "terminal.resize", ResizeTerminal => Done, [NotFound, WrongState, Rejected];

    /// The daemon's state as a one-off read, for a command that does not subscribe.
    GetDaemon, "daemon.get", GetDaemon => x100_core::fold::DaemonState, [];

    /// Changes one of the daemon's settings. Which decision model classifies is changed with
    /// `decision_model.connect` and `.disconnect` only, and Homebox and GitHub with
    /// `integrations.*`: each is `rejected` here.
    SetDaemonSetting, "daemon.set_setting", SetDaemonSetting => Done, [Rejected];

    /// Moves the clock of a daemon started with `--fake-clock` to `at`, fires every job then due,
    /// and answers once each has been acted on. For tests and scripted runs: a daemon on the real
    /// clock answers `wrong_state`, and a time before the clock's is `rejected`.
    SetClock, "daemon.set_clock", SetClock => Done, [WrongState, Rejected];

    /// Sets the most live drivers a machine may run.
    SetMachineCap, "machines.set_cap", SetMachineCap => Done, [NotFound];

    /// Looks for harnesses on this machine again, and returns every one found.
    RescanHarnesses, "harnesses.rescan", RescanHarnesses => Rescanned, [Unavailable];

    /// Adds a credential. The value goes to the vault and is never returned by any method.
    AddCredential, "credentials.add", AddCredential => CredentialAdded, [Rejected];

    /// Replaces a credential's value, keeping its reference and scope.
    ReplaceCredential, "credentials.replace", ReplaceCredential => Done, [NotFound];

    /// Sets the hosts, commands and projects a credential may be used for.
    ScopeCredential, "credentials.set_scope", ScopeCredential => Done, [NotFound];

    /// Removes a credential and its value. Removing the decision model's key disconnects it.
    RemoveCredential, "credentials.remove", RemoveCredential => Done, [NotFound];

    /// Connects a decision model to classify the actions no rule covers. The key goes to the
    /// vault, scoped to the provider's host alone and to no project, so no agent can use it, and
    /// is never returned by any method. A decision model already connected is replaced and its
    /// key removed. A key too short to tell apart in output is `rejected`.
    ConnectDecisionModel, "decision_model.connect", ConnectDecisionModel => DecisionModelConnected, [Rejected];

    /// Disconnects the decision model and removes its key. With none connected, nothing is
    /// classified: an unruled read goes through and an unruled write waits for the builder.
    DisconnectDecisionModel, "decision_model.disconnect", DisconnectDecisionModel => Done, [];

    /// Lists the integrations x100 supports, Homebox and GitHub, each with its connection if it
    /// has one and the ways it can be connected on this machine now. Finding a local Homebox and
    /// a logged-in `gh` only reads: neither key nor token is read.
    ListIntegrations, "integrations.list", ListIntegrations => IntegrationList, [];

    /// Connects Homebox, replacing one connected before. `local` runs one fixed command in the
    /// Homebox container on this machine, which mints an edit key named x100; the client asks the
    /// builder first. `address` takes a host address and an API key made there, checked with
    /// Homebox before it is kept. Either key goes to the vault, scoped to that Homebox's host and
    /// port and to no project, and is never returned by any method. No local Homebox, or a key
    /// Homebox refuses, is `rejected`.
    ConnectHomebox, "integrations.connect_homebox", ConnectHomebox => HomeboxConnected, [Rejected, Unavailable];

    /// Connects GitHub, where a project's repository is created and pushed so Homebox can deploy
    /// it. `local_gh` uses the `gh` command line logged in here, whose token the daemon never
    /// reads. `device_flow` starts GitHub's device flow and answers with the code the builder
    /// enters at GitHub; it is `unavailable` until an OAuth app for x100 is registered and its
    /// client id given to the daemon.
    ConnectGitHub, "integrations.connect_github", ConnectGitHub => GitHubConnecting, [Rejected, Unavailable];

    /// Disconnects an integration and removes its key from the vault. Projects keep their links.
    /// A key minted in a local Homebox stays there until removed in Homebox's Identities.
    DisconnectIntegration, "integrations.disconnect", DisconnectIntegration => Done, [];

    /// The connected Homebox's projects, for a project's link to be chosen from. With none
    /// connected, `wrong_state`; when Homebox cannot be asked, `unavailable`.
    ListHomeboxProjects, "integrations.homebox_projects", ListHomeboxProjects => HomeboxProjects, [WrongState, Unavailable];

    /// Lists projects with the sidebar's line for each.
    ListProjects, "projects.list", ListProjects => ProjectList, [];

    /// One project's state as a one-off read.
    GetProject, "projects.get", GetProject => x100_core::fold::ProjectState, [NotFound];

    /// Creates a project, and with it the CEO. A new project has no name, no intent and no code
    /// directory: the CEO sets it up once the builder's messages say what it is. With `dir`, it
    /// starts on that existing folder, which is its code directory from the start, and a git
    /// repository is started there if there is none. `settings` defaults to the daemon's. A client
    /// creates one as the builder sends its first message, then sends that message.
    CreateProject, "projects.create", CreateProject => ProjectCreated, [Rejected, Unavailable];

    /// Renames a project that is set up. Its intent and its code directory stay.
    RenameProject, "projects.rename", RenameProject => Done, [NotFound, WrongState, Rejected];

    /// Changes a project setting: Ask me about, the total budget, the lab's share, or whether a
    /// harness and model are allowed. Turning one off hands the work of drivers using it on. The
    /// Homebox link is changed with `projects.link_homebox` and `.unlink_homebox`, and the
    /// repository is x100's to record: each is `rejected` here.
    ChangeProjectSetting, "projects.set", ChangeProjectSetting => Done, [NotFound, Rejected];

    /// Links a project to a project of the connected Homebox, found by its name or its
    /// repository's full name, deploying to `environment` (production unless named). A name that
    /// finds none or several is `rejected` with the choices.
    LinkHomebox, "projects.link_homebox", LinkHomebox => HomeboxLinked, [NotFound, WrongState, Rejected, Unavailable];

    /// Removes a project's link to Homebox.
    UnlinkHomebox, "projects.unlink_homebox", UnlinkHomebox => Done, [NotFound];

    /// The builder deploys a project's main branch through Homebox, and the answer comes when it
    /// is live or has failed. A project not on GitHub yet gets a private repository first, is
    /// added to Homebox and linked. Without Homebox and GitHub connected it is `wrong_state`, and
    /// a deploy that fails is `rejected` with why.
    DeployProject, "projects.deploy", DeployProject => ProjectDeployed, [NotFound, WrongState, Rejected];

    /// Reflect now: delegates a reflection task to the CEO.
    Reflect, "projects.reflect", Reflect => Reflecting, [NotFound, WrongState, Unavailable];

    /// Sends a message to the CEO, with files already uploaded attached. The builder talks to no
    /// one else. Once the project is set up, a message opens a task for the CEO when it holds none
    /// from the builder; before, it is only a message, which the CEO answers.
    SendChat, "chat.send", SendChat => ChatSent, [NotFound];

    /// Raises a task as a new idea.
    RaiseTask, "tasks.raise", RaiseTask => TaskRaised, [NotFound];

    /// Uploads a file's bytes (base64 in `data`) and records it. Returns the file with its hash, to
    /// attach to a message or a task.
    UploadFile, "files.upload", UploadFile => FileUploaded, [NotFound, TooLarge];

    /// Reads a file's bytes.
    ReadFile, "files.read", ReadFile => FileContent, [NotFound];

    /// Removes a file to save space. The removal is logged.
    RemoveFile, "files.remove", RemoveFile => Done, [NotFound, WrongState];

    /// Cancels a scheduled job. A job that has fired, or was cancelled, is `wrong_state`.
    CancelJob, "jobs.cancel", CancelJob => Done, [NotFound, WrongState];

    /// Approves or denies a held action, and returns once what it calls for is done: on Approve a
    /// held tool call is carried out (a deploy is started, and its result reaches the agent when it
    /// is done), a held merge is done, and a held command or request out is allowed once, for the
    /// agent's next try; on Deny nothing runs, and a held merge's task goes
    /// back to its assignee. Either way the agent is told through its inbox. An action that is not
    /// held is `wrong_state`.
    DecideAction, "actions.decide", DecideAction => Done, [NotFound, WrongState];

    /// Sets a lead's budget. All leads' budgets together may not pass the project's total.
    SetLeadBudget, "agents.set_budget", SetLeadBudget => Done, [NotFound, Rejected];

    /// Sets an agent's effort range. A driver outside the new range is handed on.
    SetEffortRange, "agents.set_effort", SetEffortRange => Done, [NotFound, Rejected];

    /// Sets how an agent looks: the outline of its face and its colour. A retired agent is
    /// `wrong_state`.
    SetAppearance, "agents.set_appearance", SetAppearance => Done, [NotFound, WrongState];

    /// Replaces the driver acting for an agent. The work hands off the way any handoff does.
    ReplaceDriver, "agents.replace_driver", ReplaceDriver => Done, [NotFound, WrongState];

    /// What has been delivered to an agent and not yet handled.
    ReadInbox, "agents.inbox", ReadInbox => Inbox, [NotFound];

    /// What an agent is waiting for, each with its deadline.
    ReadWaits, "agents.waits", ReadWaits => Waits, [NotFound];

    /// Stops a driver. Its work is handed to another.
    StopDriver, "drivers.stop", StopDriver => Done, [NotFound, WrongState];

    /// The start pack a driver was given.
    ReadStartPack, "drivers.start_pack", ReadStartPack => StartPack, [NotFound];

    /// An agent's memory notes with their text.
    ListMemories, "memories.list", ListMemories => Memories, [NotFound];

    /// Skills with their text: shared ones, or one scope's.
    ListSkills, "skills.list", ListSkills => Skills, [NotFound];

    /// Reflection's trials with both arms' runs and scores.
    ListTrials, "trials.list", ListTrials => Trials, [NotFound];
}

macro_rules! notices {
    ($(
        $(#[doc = $doc:literal])+
        $variant:ident, $name:literal, $body:ty;
    )*) => {
        /// A message the daemon sends without being asked.
        #[derive(Debug, Clone, PartialEq)]
        pub enum Notice {
            $($variant($body),)*
        }

        /// Every notification, in the order `PROTOCOL.md` lists them.
        pub const NOTICES: &[(&str, &str)] = &[$(($name, concat!($($doc, "\n"),+))),*];

        impl Notice {
            pub fn method(&self) -> &'static str {
                match self {
                    $(Notice::$variant(_) => $name,)*
                }
            }

            pub fn params(&self) -> Value {
                match self {
                    $(Notice::$variant(p) => serde_json::to_value(p).expect("notice serialises"),)*
                }
            }

            pub fn parse(method: &str, params: Value) -> Result<Notice, RpcError> {
                match method {
                    $($name => serde_json::from_value(params)
                        .map(Notice::$variant)
                        .map_err(|e| RpcError::new(ErrorCode::InvalidParams, e.to_string())),)*
                    other => Err(RpcError::new(
                        ErrorCode::MethodNotFound,
                        format!("no notification `{other}`"),
                    )),
                }
            }
        }
    };
}

notices! {
    /// One event of the daemon's log or a project's, on an events subscription.
    Event, "events.event", EventNotice;

    /// One entry of an agent's transcript, on a transcript subscription.
    TranscriptEntry, "transcript.entry", TranscriptNotice;

    /// One chunk of a driver's terminal, on a terminal subscription.
    TerminalChunk, "terminal.chunk", TerminalNotice;

    /// The daemon ended a subscription itself.
    Closed, "subscription.closed", SubscriptionClosed;
}
