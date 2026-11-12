//! The method table. One entry per method gives its name, params type, result type and the errors
//! it can return, and the doc comment is the method's summary in `PROTOCOL.md`. The macro makes
//! [`Call`], the typed request, so a method cannot exist without all four.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{ErrorCode, RpcError};
use crate::types::*;
use crate::{EventNotice, SubscriptionClosed, TranscriptNotice};

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

    /// Ends a subscription, of events or of a transcript.
    Unsubscribe, "unsubscribe", Unsubscribe => Done, [UnknownSubscription];

    /// Reads a page of the daemon's log or a project's: newest first for the Log tab, or oldest
    /// first from a point.
    ReadLog, "events.read", ReadLog => LogPage, [NotFound];

    /// Reads a page of an agent's transcript, after a position.
    ReadTranscript, "transcript.read", ReadTranscript => TranscriptPage, [NotFound];

    /// Follows an agent's transcript: the entries after `after` are notified first, then new ones as
    /// they are written, with none missed or repeated.
    SubscribeTranscript, "transcript.subscribe", SubscribeTranscript => TranscriptSubscribed, [NotFound];

    /// The daemon's state as a one-off read, for a command that does not subscribe.
    GetDaemon, "daemon.get", GetDaemon => x100_core::fold::DaemonState, [];

    /// Changes one of the daemon's settings.
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

    /// Removes a credential and its value.
    RemoveCredential, "credentials.remove", RemoveCredential => Done, [NotFound];

    /// Lists projects with the sidebar's line for each.
    ListProjects, "projects.list", ListProjects => ProjectList, [];

    /// One project's state as a one-off read.
    GetProject, "projects.get", GetProject => x100_core::fold::ProjectState, [NotFound];

    /// Creates a project in a directory, starting a git repository there if there is none, and with
    /// it the CEO. `settings` defaults to the daemon's.
    CreateProject, "projects.create", CreateProject => ProjectCreated, [Rejected, Unavailable];

    /// Changes a project setting: Ask me about, the total budget, the lab's share, or whether a
    /// harness and model are allowed. Turning one off hands the work of drivers using it on.
    ChangeProjectSetting, "projects.set", ChangeProjectSetting => Done, [NotFound, Rejected];

    /// Reflect now: delegates a reflection task to the CEO.
    Reflect, "projects.reflect", Reflect => Reflecting, [NotFound, WrongState, Unavailable];

    /// Sends the brief, the project's first message, with files already uploaded attached. It is
    /// delegated to the CEO as a task.
    SendBrief, "chat.brief", SendBrief => BriefSent, [NotFound, WrongState];

    /// Sends a message to the CEO. The builder talks to no one else.
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

    /// Approves or denies a held action. An action that is not held is `wrong_state`.
    DecideAction, "actions.decide", DecideAction => Done, [NotFound, WrongState];

    /// Sets a lead's budget. All leads' budgets together may not pass the project's total.
    SetLeadBudget, "agents.set_budget", SetLeadBudget => Done, [NotFound, Rejected];

    /// Sets an agent's effort range. A driver outside the new range is handed on.
    SetEffortRange, "agents.set_effort", SetEffortRange => Done, [NotFound, Rejected];

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

    /// The daemon ended a subscription itself.
    Closed, "subscription.closed", SubscriptionClosed;
}
