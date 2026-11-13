//! The typed calls the interfaces' tabs make beyond the connection itself. Each is one request with
//! the protocol's own params; none holds state or logic.

use x100_core::action::Decision;
use x100_core::appearance::Appearance;
use x100_core::credential::CredentialScope;
use x100_core::decision::DecisionProvider;
use x100_core::effort::EffortRange;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::id::{ActionId, AgentId, CredentialRef, DriverId, FileId, ProjectId, TaskId};
use x100_core::money::Money;
use x100_core::project::SettingChange;
use x100_proto::{
    AddCredential, ChangeProjectSetting, ConnectDecisionModel, DecideAction,
    DisconnectDecisionModel, ListMemories, ListSkills, ListTrials, LogId, LogPage, Memories, Page,
    RaiseTask, ReadLog, RemoveCredential, RemoveFile, RenameProject, ReplaceCredential,
    ResizeTerminal, ScopeCredential, Secret, SetAppearance, SetEffortRange, SetLeadBudget,
    SkillWritten, Skills, Trials, WriteSkill,
};

use crate::{Client, ClientError};

impl Client {
    /// Renames a project its CEO has set up.
    pub async fn rename_project(
        &self,
        project: ProjectId,
        name: impl Into<String>,
    ) -> Result<(), ClientError> {
        self.request(RenameProject {
            project,
            name: name.into(),
        })
        .await
        .map(drop)
    }

    /// Tells a live terminal driver how many cells the viewer's pane holds, so its program redraws
    /// at that size.
    pub async fn resize_terminal(
        &self,
        project: ProjectId,
        driver: DriverId,
        cols: u16,
        rows: u16,
    ) -> Result<(), ClientError> {
        self.request(ResizeTerminal {
            project,
            driver,
            cols,
            rows,
        })
        .await
        .map(drop)
    }

    /// A page of a project's log, in the order `page` asks for.
    pub async fn read_events(
        &self,
        project: ProjectId,
        page: Page,
    ) -> Result<Vec<Envelope<ProjectEvent>>, ClientError> {
        match self
            .request(ReadLog {
                log: LogId::Project(project),
                page,
            })
            .await?
        {
            LogPage::Project { events } => Ok(events),
            LogPage::Daemon { .. } => Err(ClientError::Protocol(
                "a project's log came back as the daemon's".into(),
            )),
        }
    }

    /// Up to `limit` events below `before`, newest first: the Log tab's pages.
    pub async fn read_events_before(
        &self,
        project: ProjectId,
        before: Option<Seq>,
        limit: u32,
    ) -> Result<Vec<Envelope<ProjectEvent>>, ClientError> {
        self.read_events(project, Page::NewestFirst { before, limit })
            .await
    }

    /// Up to `limit` events above `after`, oldest first: what a log follows with.
    pub async fn read_events_after(
        &self,
        project: ProjectId,
        after: Option<Seq>,
        limit: u32,
    ) -> Result<Vec<Envelope<ProjectEvent>>, ClientError> {
        self.read_events(project, Page::OldestFirst { after, limit })
            .await
    }

    /// Raises a task as a new idea.
    pub async fn raise_task(
        &self,
        project: ProjectId,
        title: impl Into<String>,
        ask: impl Into<String>,
        part: Option<String>,
    ) -> Result<TaskId, ClientError> {
        let raised = self
            .request(RaiseTask {
                project,
                title: title.into(),
                ask: ask.into(),
                part,
                attachments: Vec::new(),
            })
            .await?;
        Ok(raised.task)
    }

    pub async fn remove_file(&self, project: ProjectId, file: FileId) -> Result<(), ClientError> {
        self.request(RemoveFile { project, file }).await?;
        Ok(())
    }

    /// Changes Ask me about, the total budget, the lab's share, or a harness and model allowed.
    pub async fn change_setting(
        &self,
        project: ProjectId,
        change: SettingChange,
    ) -> Result<(), ClientError> {
        self.request(ChangeProjectSetting { project, change })
            .await?;
        Ok(())
    }

    pub async fn set_lead_budget(
        &self,
        project: ProjectId,
        lead: AgentId,
        budget: Money,
    ) -> Result<(), ClientError> {
        self.request(SetLeadBudget {
            project,
            lead,
            budget,
        })
        .await?;
        Ok(())
    }

    pub async fn set_effort_range(
        &self,
        project: ProjectId,
        agent: AgentId,
        range: EffortRange,
    ) -> Result<(), ClientError> {
        self.request(SetEffortRange {
            project,
            agent,
            range,
        })
        .await?;
        Ok(())
    }

    /// Changes how an agent looks.
    pub async fn set_appearance(
        &self,
        project: ProjectId,
        agent: AgentId,
        appearance: Appearance,
    ) -> Result<(), ClientError> {
        self.request(SetAppearance {
            project,
            agent,
            appearance,
        })
        .await?;
        Ok(())
    }

    /// Approves or denies a held action.
    pub async fn decide_action(
        &self,
        project: ProjectId,
        action: ActionId,
        decision: Decision,
    ) -> Result<(), ClientError> {
        self.request(DecideAction {
            project,
            action,
            decision,
        })
        .await?;
        Ok(())
    }

    /// Adds a credential. The value goes to the daemon's vault and is not held here.
    pub async fn add_credential(
        &self,
        name: impl Into<String>,
        value: impl Into<String>,
        scope: CredentialScope,
    ) -> Result<CredentialRef, ClientError> {
        let added = self
            .request(AddCredential {
                name: name.into(),
                value: Secret::new(value),
                scope,
            })
            .await?;
        Ok(added.credential)
    }

    pub async fn replace_credential(
        &self,
        credential: CredentialRef,
        value: impl Into<String>,
    ) -> Result<(), ClientError> {
        self.request(ReplaceCredential {
            credential,
            value: Secret::new(value),
        })
        .await?;
        Ok(())
    }

    pub async fn scope_credential(
        &self,
        credential: CredentialRef,
        scope: CredentialScope,
    ) -> Result<(), ClientError> {
        self.request(ScopeCredential { credential, scope }).await?;
        Ok(())
    }

    pub async fn remove_credential(&self, credential: CredentialRef) -> Result<(), ClientError> {
        self.request(RemoveCredential { credential }).await?;
        Ok(())
    }

    /// Connects a decision model to classify. The key goes to the daemon's vault and is not held
    /// here.
    pub async fn connect_decision_model(
        &self,
        provider: DecisionProvider,
        key: impl Into<String>,
    ) -> Result<CredentialRef, ClientError> {
        let connected = self
            .request(ConnectDecisionModel {
                provider,
                key: Secret::new(key),
            })
            .await?;
        Ok(connected.credential)
    }

    pub async fn disconnect_decision_model(&self) -> Result<(), ClientError> {
        self.request(DisconnectDecisionModel {}).await?;
        Ok(())
    }

    pub async fn list_memories(
        &self,
        project: ProjectId,
        agent: AgentId,
    ) -> Result<Memories, ClientError> {
        self.request(ListMemories { project, agent }).await
    }

    /// The skills of one scope, or all of them for `None`.
    pub async fn list_skills(
        &self,
        project: ProjectId,
        scope: Option<x100_core::memory::SkillScope>,
    ) -> Result<Skills, ClientError> {
        self.request(ListSkills { project, scope }).await
    }

    /// Writes a skill, new or changed, to the main branch as the builder.
    pub async fn write_skill(
        &self,
        project: ProjectId,
        skill: x100_core::memory::SkillRef,
        text: String,
    ) -> Result<SkillWritten, ClientError> {
        self.request(WriteSkill {
            project,
            skill,
            text,
        })
        .await
    }

    pub async fn list_trials(&self, project: ProjectId) -> Result<Trials, ClientError> {
        self.request(ListTrials { project }).await
    }
}
