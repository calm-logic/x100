//! Methods on a project and what is in it.

use serde::{Deserialize, Serialize};
use x100_core::action::Decision;
use x100_core::appearance::Appearance;
use x100_core::effort::EffortRange;
use x100_core::file::FileRef;
use x100_core::git::CommitId;
use x100_core::id::{
    ActionId, AgentId, DriverId, FileId, InboxItemId, JobId, MessageId, ProjectId, TaskId,
};
use x100_core::inbox::InboxItem;
use x100_core::memory::{NoteName, SkillRef, SkillScope, SkillVersion};
use x100_core::money::Money;
use x100_core::project::{SettingChange, Settings};
use x100_core::wait::Wait;

use x100_core::bytes::Bytes;

/// Lists the projects with the sidebar's line for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListProjects {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub project: ProjectId,
    /// `None` until the CEO sets the project up.
    pub name: Option<String>,
    /// The code directory, once it has one.
    pub dir: Option<String>,
    /// An agent is at work.
    pub live: bool,
    pub agents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectList {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetProject {
    pub project: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProject {
    /// An existing folder to start the project on, as an absolute path: its code directory from
    /// the start, where a git repository is started if it has none. Without it the project has
    /// no code until its CEO sets it up.
    pub dir: Option<String>,
    /// Defaults to the daemon's.
    pub settings: Option<Settings>,
}

/// The builder renames a project its CEO has set up. The intent and the code directory stay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameProject {
    pub project: ProjectId,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreated {
    pub project: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeProjectSetting {
    pub project: ProjectId,
    pub change: SettingChange,
}

/// Reflect now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reflect {
    pub project: ProjectId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reflecting {
    /// The reflection task the builder delegated to the CEO.
    pub task: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendChat {
    pub project: ProjectId,
    pub body: String,
    pub attachments: Vec<FileId>,
    /// Set when answering a question the CEO asked.
    pub in_reply_to: Option<MessageId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSent {
    pub message: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaiseTask {
    pub project: ProjectId,
    pub title: String,
    pub ask: String,
    pub part: Option<String>,
    pub attachments: Vec<FileId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRaised {
    pub task: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadFile {
    pub project: ProjectId,
    pub name: String,
    pub media_type: String,
    pub data: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileUploaded {
    pub file: FileRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFile {
    pub project: ProjectId,
    pub file: FileId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    pub file: FileRef,
    pub data: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveFile {
    pub project: ProjectId,
    pub file: FileId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelJob {
    pub project: ProjectId,
    pub job: JobId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideAction {
    pub project: ProjectId,
    pub action: ActionId,
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetLeadBudget {
    pub project: ProjectId,
    pub lead: AgentId,
    pub budget: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetEffortRange {
    pub project: ProjectId,
    pub agent: AgentId,
    pub range: EffortRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetAppearance {
    pub project: ProjectId,
    pub agent: AgentId,
    pub appearance: Appearance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceDriver {
    pub project: ProjectId,
    pub agent: AgentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopDriver {
    pub project: ProjectId,
    pub driver: DriverId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadStartPack {
    pub project: ProjectId,
    pub driver: DriverId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartPack {
    /// The pack as the driver was given it.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadInbox {
    pub project: ProjectId,
    pub agent: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEntry {
    pub id: InboxItemId,
    pub item: InboxItem,
    /// Whether a turn is handling it now.
    pub being_handled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub items: Vec<InboxEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadWaits {
    pub project: ProjectId,
    pub agent: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waits {
    pub waits: Vec<Wait>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMemories {
    pub project: ProjectId,
    pub agent: AgentId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryNote {
    pub name: NoteName,
    /// The commit that last wrote it.
    pub commit: CommitId,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memories {
    pub notes: Vec<MemoryNote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSkills {
    pub project: ProjectId,
    /// Only this scope's skills; all of them for `None`.
    pub scope: Option<SkillScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInfo {
    pub skill: SkillRef,
    pub version: SkillVersion,
    /// The skill's `SKILL.md`.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skills {
    pub skills: Vec<SkillInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTrials {
    pub project: ProjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trials {
    pub trials: Vec<x100_core::fold::TrialRecord>,
}
