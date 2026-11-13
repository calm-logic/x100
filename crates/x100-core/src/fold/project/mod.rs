//! The fold of one project's log into what its tabs and views show.

mod team;
mod work;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::records::{
    ActionRecord, AgentRecord, ChatEntry, DriverRecord, FileRecord, JobRecord, MessageRecord,
    SkillRecord, SpecRecord, Spent, TaskRecord, TrialRecord,
};
use super::{Changes, Corrupt, Fail, FoldError, Record, expected_after, on, unknown};
use crate::agent::AgentState;
use crate::appearance::Appearance;
use crate::effort::EffortRange;
use crate::event::{Envelope, ProjectEvent, Seq};
use crate::id::{ActionId, AgentId, DriverId, FileId, JobId, MessageId, TaskId, TrialId};
use crate::job::ProjectJob;
use crate::ledger::{Budgets, Spend};
use crate::money::Usage;
use crate::project::{Identity, SettingChange, Settings};
use crate::spec::SpecKind;
use crate::team::{Team, TeamOp};
use crate::time::Timestamp;

/// The parts of a project's interface an event can change: the main panel's tabs, and the chat,
/// the held actions and the project's line in the sidebar. An agent's view is part of Team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPart {
    Team,
    Specs,
    Tasks,
    Files,
    Budget,
    Log,
    Preview,
    Settings,
    Chat,
    HeldActions,
    Sidebar,
    /// An agent's look changed: everything that draws its face, in any tab.
    Looks,
}

/// The project itself, once its log has created it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Its name and intent, from the moment the CEO sets it up. `None` while it is not set up.
    pub identity: Option<Identity>,
    /// Where its repository is. `None` until it is set up, unless it started on an existing
    /// folder.
    pub code_dir: Option<PathBuf>,
    pub ceo: AgentId,
    pub created: Timestamp,
    pub settings: Settings,
    /// The address of the development build, from `set_preview`.
    pub preview: Option<String>,
    /// The tree, each agent's effort range and each lead's budget.
    pub team: Team,
    /// The Homebox project it deploys to, once linked.
    pub homebox: Option<crate::integration::HomeboxLink>,
    /// The repository on GitHub its main branch is pushed to, once x100 published it.
    pub repository: Option<String>,
}

impl Project {
    /// The name the CEO gave it, once it is set up.
    pub fn name(&self) -> Option<&str> {
        self.identity.as_ref().map(|i| i.name.as_str())
    }

    pub fn is_set_up(&self) -> bool {
        self.identity.is_some()
    }
}

/// Everything every tab and view of one project draws, folded from its log.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProjectState {
    pub(super) seq: Option<Seq>,
    pub(super) project: Option<Project>,
    pub(super) agents: BTreeMap<AgentId, AgentRecord>,
    pub(super) drivers: BTreeMap<DriverId, DriverRecord>,
    pub(super) tasks: BTreeMap<TaskId, TaskRecord>,
    pub(super) messages: BTreeMap<MessageId, MessageRecord>,
    pub(super) chat: Vec<ChatEntry>,
    pub(super) jobs: BTreeMap<JobId, JobRecord<ProjectJob>>,
    pub(super) actions: BTreeMap<ActionId, ActionRecord>,
    pub(super) specs: BTreeMap<SpecKind, SpecRecord>,
    pub(super) files: BTreeMap<FileId, FileRecord>,
    pub(super) trials: BTreeMap<TrialId, TrialRecord>,
    pub(super) skills: Vec<SkillRecord>,
    pub(super) spend: Spend,
    pub(super) usage: Usage,
}

type Ch = Changes<ProjectPart>;

/// Where an event sits in its log and when it happened.
#[derive(Clone, Copy)]
pub(super) struct Stamp {
    pub(super) seq: Seq,
    pub(super) at: Timestamp,
}

impl AgentRecord {
    fn new(
        id: AgentId,
        role: &str,
        charter: &str,
        joined: Timestamp,
        appearance: Appearance,
    ) -> Self {
        Self {
            id,
            role: role.to_owned(),
            charter: charter.to_owned(),
            joined,
            appearance,
            state: AgentState::INITIAL,
            inbox: Default::default(),
            items: BTreeMap::new(),
            waits: BTreeMap::new(),
            open_tasks: Vec::new(),
            drivers: Vec::new(),
            memories: BTreeMap::new(),
            skills: Vec::new(),
            processes: BTreeMap::new(),
            spent: Spent::default(),
        }
    }
}

impl ProjectState {
    /// Folds a whole log.
    pub fn fold<'a>(
        log: impl IntoIterator<Item = &'a Envelope<ProjectEvent>>,
    ) -> Result<Self, FoldError> {
        let mut state = Self::default();
        for e in log {
            state.apply(e)?;
        }
        Ok(state)
    }

    /// Applies the next event of the log and says what it changed. Events must come in order,
    /// each once.
    pub fn apply(&mut self, e: &Envelope<ProjectEvent>) -> Result<Changes<ProjectPart>, FoldError> {
        let expected = expected_after(self.seq);
        if e.seq != expected {
            return Err(FoldError {
                seq: e.seq,
                record: None,
                cause: Corrupt::OutOfOrder { expected },
            });
        }
        let mut ch = Changes::new();
        ch.part(ProjectPart::Log);
        let stamp = Stamp {
            seq: e.seq,
            at: e.at,
        };
        self.event(stamp, &e.event, &mut ch)
            .map_err(|(record, cause)| FoldError {
                seq: e.seq,
                record,
                cause,
            })?;
        self.seq = Some(e.seq);
        Ok(ch)
    }

    fn event(&mut self, s: Stamp, event: &ProjectEvent, ch: &mut Ch) -> Result<(), Fail> {
        use ProjectEvent as P;
        if self.project.is_none() && !matches!(event, P::Created { .. }) {
            return Err((None, Corrupt::NotCreated));
        }
        match event {
            P::Created {
                ceo,
                charter,
                effort,
                settings,
            } => self.create(s.at, *ceo, charter, *effort, settings, ch),
            P::CodeDirSet { dir } => self.code_dir_set(dir, ch),
            P::SetUp(identity) => self.identify(identity, false, ch),
            P::Renamed(identity) => self.identify(identity, true, ch),
            P::SettingChanged(change) => self.setting(change, ch),
            P::PreviewSet { url } => {
                self.project_mut()?.preview = Some(url.clone());
                ch.part(ProjectPart::Preview);
                Ok(())
            }
            P::AgentAdded(a) => self.agent_added(s, a, ch),
            P::Agent { agent, event: e } => self.agent_event(*agent, e, event, ch),
            P::DriverStarted(d) => self.driver_started(s, d, ch),
            P::Driver { driver, event } => self.driver_event(s.at, *driver, event, ch),
            P::ProcessStarted(p) => self.process_started(s, p, ch),
            P::ProcessStopped { agent, name, .. } => self.process_stopped(agent, name, ch),
            P::Checkpointed { agent, .. } | P::BranchRefreshed { agent, .. } => {
                self.agent_mut(*agent).map(|_| ())
            }
            P::OneOffCalled(_) | P::CredentialUsed(_) => Ok(()),
            P::Charged(c) => self.charged(c, ch),
            P::BudgetReached(_) => {
                ch.part(ProjectPart::Budget);
                Ok(())
            }
            P::TaskOpened { task, opening } => self.task_opened(s.at, task, opening, ch),
            P::Task { task, event } => self.task_event(s, *task, event, ch),
            P::MessageSent(m) => self.message_sent(s, m, ch),
            P::JobScheduled(j) => self.job_scheduled(s, j, ch),
            P::Job { job, event } => self.job_event(job, event, ch),
            P::ActionProposed(a) => self.action_proposed(s, a, ch),
            P::Action { action, event } => self.action_event(s.at, *action, event, ch),
            P::SpecWritten(spec) => self.spec_written(s, spec, ch),
            P::FileAdded(f) => self.file_added(s, f, ch),
            P::FileRemoved { file } => self.file_removed(file, ch),
            P::MemoryWritten {
                agent,
                note,
                commit,
            } => self.memory_written(agent, note, commit, ch),
            P::SkillLoaded {
                agent,
                skill,
                version,
            } => self.skill_loaded(agent, skill, version, ch),
            P::TrialProposed(t) => self.trial_proposed(s, t, ch),
            P::Trial { trial, event } => self.trial_event(trial, event, ch),
            P::SkillCommitted {
                skill,
                version,
                commit,
                trial,
            } => self.skill_committed(skill, version, commit, trial, ch),
            P::MemoryCompacted { agent, trial, .. } => self.memory_compacted(agent, trial, ch),
        }
    }

    fn create(
        &mut self,
        at: Timestamp,
        ceo: AgentId,
        charter: &str,
        effort: EffortRange,
        settings: &Settings,
        ch: &mut Ch,
    ) -> Result<(), Fail> {
        if self.project.is_some() {
            return Err((None, Corrupt::AlreadyCreated));
        }
        self.project = Some(Project {
            identity: None,
            code_dir: None,
            ceo,
            created: at,
            settings: settings.clone(),
            preview: None,
            team: Team::new(ceo, effort),
            homebox: None,
            repository: None,
        });
        self.agents.insert(
            ceo,
            AgentRecord::new(ceo, "CEO", charter, at, Appearance::default()),
        );
        for part in [
            ProjectPart::Team,
            ProjectPart::Budget,
            ProjectPart::Settings,
            ProjectPart::Chat,
            ProjectPart::Sidebar,
        ] {
            ch.part(part);
        }
        ch.record(Record::Agent(ceo));
        Ok(())
    }

    fn code_dir_set(&mut self, dir: &Path, ch: &mut Ch) -> Result<(), Fail> {
        let project = self.project_mut()?;
        if project.code_dir.is_some() {
            return Err((None, Corrupt::CodeDirAlreadySet));
        }
        project.code_dir = Some(dir.to_owned());
        ch.part(ProjectPart::Settings).part(ProjectPart::Files);
        Ok(())
    }

    /// The first naming sets the project up; a later one renames it. Neither moves anything.
    fn identify(&mut self, identity: &Identity, rename: bool, ch: &mut Ch) -> Result<(), Fail> {
        let project = self.project_mut()?;
        match (rename, project.is_set_up()) {
            (false, true) => return Err((None, Corrupt::AlreadySetUp)),
            (true, false) => return Err((None, Corrupt::NotSetUp)),
            _ => {}
        }
        project.identity = Some(identity.clone());
        for part in [
            ProjectPart::Settings,
            ProjectPart::Chat,
            ProjectPart::Sidebar,
        ] {
            ch.part(part);
        }
        Ok(())
    }

    fn setting(&mut self, change: &SettingChange, ch: &mut Ch) -> Result<(), Fail> {
        let project = self.project_mut()?;
        let mut settings = project.settings.clone();
        match change {
            SettingChange::AskMeAbout(level) => settings.ask_me_about = *level,
            SettingChange::Budget(total) => settings.budget = *total,
            SettingChange::LabShare(share) => settings.lab_share = *share,
            SettingChange::Allow { what, allowed } => {
                if *allowed {
                    settings.allowed.insert(what.clone());
                } else {
                    settings.allowed.remove(what);
                }
            }
            SettingChange::Homebox(link) => project.homebox = link.clone(),
            SettingChange::Repository(full_name) => project.repository = Some(full_name.clone()),
        }
        Budgets::in_force(&settings, &project.team)
            .check_leads()
            .map_err(|e| (None, e.into()))?;
        project.settings = settings;
        ch.part(ProjectPart::Settings);
        if matches!(
            change,
            SettingChange::Budget(_) | SettingChange::LabShare(_)
        ) {
            ch.part(ProjectPart::Budget);
        }
        Ok(())
    }

    /// Applies a change to the team's shape, refusing one that breaks the tree or puts the leads'
    /// budgets over the project's total.
    pub(super) fn change_team(&mut self, op: &TeamOp, record: Record) -> Result<(), Fail> {
        let project = self.project_mut()?;
        let team = project.team.apply(op).map_err(on(record))?;
        Budgets::in_force(&project.settings, &team)
            .check_leads()
            .map_err(on(record))?;
        project.team = team;
        Ok(())
    }

    pub(super) fn project_mut(&mut self) -> Result<&mut Project, Fail> {
        self.project.as_mut().ok_or((None, Corrupt::NotCreated))
    }

    pub(super) fn agent_mut(&mut self, id: AgentId) -> Result<&mut AgentRecord, Fail> {
        self.agents.get_mut(&id).ok_or(unknown(Record::Agent(id)))
    }
}
