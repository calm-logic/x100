//! What each tab and view reads from a project's state.

use std::collections::BTreeMap;

use super::project::{Project, ProjectState};
use super::records::{
    ActionRecord, AgentRecord, ChatEntry, DriverRecord, FileRecord, JobRecord, MessageRecord,
    SkillRecord, SpecRecord, Spent, TaskRecord, TrialRecord,
};
use crate::action::ActionState;
use crate::agent::{AgentState, Position};
use crate::driver::Purpose;
use crate::effort::EffortRange;
use crate::event::Seq;
use crate::id::{ActionId, AgentId, DriverId, FileId, JobId, MessageId, TaskId, TrialId};
use crate::job::{JobState, ProjectJob};
use crate::ledger::{Budgets, Payer};
use crate::memory::SkillScope;
use crate::message::Route;
use crate::money::{Money, Percent, Usage};
use crate::spec::SpecKind;
use crate::task::TaskState;
use crate::trial::TrialChange;

/// One row of the Team tab: an agent, what it is on, and what drives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamRow<'a> {
    pub agent: &'a AgentRecord,
    pub position: &'a Position,
    pub parent: Option<AgentId>,
    /// The range in force, which for a worker without its own is its parent's.
    pub effort: EffortRange,
    pub task: Option<&'a TaskRecord>,
    pub progress: Option<Percent>,
    pub driver: Option<&'a DriverRecord>,
}

/// Spend against a budget, and whether that budget now stops new turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetLine {
    pub spent: Money,
    pub budget: Money,
    pub stopped: bool,
}

/// The Budget tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetView {
    pub project: BudgetLine,
    pub lab: BudgetLine,
    /// Every lead the team has had. A retired lead's line keeps its spend and stops.
    pub leads: BTreeMap<AgentId, BudgetLine>,
    pub agents: BTreeMap<AgentId, Spent>,
    pub usage: Usage,
}

impl ProjectState {
    /// The last event applied, from which a client follows the log.
    pub fn seq(&self) -> Option<Seq> {
        self.seq
    }

    /// `None` until the log creates the project.
    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    pub fn agents(&self) -> impl Iterator<Item = &AgentRecord> {
        self.agents.values()
    }

    pub fn agent(&self, id: AgentId) -> Option<&AgentRecord> {
        self.agents.get(&id)
    }

    /// Every driver the project has had, live or not.
    pub fn drivers(&self) -> impl Iterator<Item = &DriverRecord> {
        self.drivers.values()
    }

    pub fn driver(&self, id: DriverId) -> Option<&DriverRecord> {
        self.drivers.get(&id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &TaskRecord> {
        self.tasks.values()
    }

    pub fn task(&self, id: TaskId) -> Option<&TaskRecord> {
        self.tasks.get(&id)
    }

    /// Messages between agents, and between the builder and the CEO.
    pub fn messages(&self) -> impl Iterator<Item = &MessageRecord> {
        self.messages.values()
    }

    pub fn message(&self, id: MessageId) -> Option<&MessageRecord> {
        self.messages.get(&id)
    }

    /// The chat with the CEO, oldest first.
    pub fn chat(&self) -> &[ChatEntry] {
        &self.chat
    }

    pub fn jobs(&self) -> impl Iterator<Item = &JobRecord<ProjectJob>> {
        self.jobs.values()
    }

    pub fn job(&self, id: JobId) -> Option<&JobRecord<ProjectJob>> {
        self.jobs.get(&id)
    }

    pub fn actions(&self) -> impl Iterator<Item = &ActionRecord> {
        self.actions.values()
    }

    pub fn action(&self, id: ActionId) -> Option<&ActionRecord> {
        self.actions.get(&id)
    }

    pub fn specs(&self) -> impl Iterator<Item = &SpecRecord> {
        self.specs.values()
    }

    pub fn spec(&self, kind: SpecKind) -> Option<&SpecRecord> {
        self.specs.get(&kind)
    }

    /// The files the project holds now. A removed file is gone from here and stays in the Log.
    pub fn files(&self) -> impl Iterator<Item = &FileRecord> {
        self.files.values()
    }

    pub fn file(&self, id: FileId) -> Option<&FileRecord> {
        self.files.get(&id)
    }

    pub fn trials(&self) -> impl Iterator<Item = &TrialRecord> {
        self.trials.values()
    }

    pub fn trial(&self, id: TrialId) -> Option<&TrialRecord> {
        self.trials.get(&id)
    }

    /// Skill changes that reached a main branch, by a trial or directly, the latest per skill.
    pub fn skills(&self) -> &[SkillRecord] {
        &self.skills
    }

    /// The driver acting for `agent` now: its live work driver. Lab drivers never act for it.
    pub fn acting_driver(&self, agent: AgentId) -> Option<DriverId> {
        let a = self.agents.get(&agent)?;
        a.drivers.iter().rev().copied().find(|d| {
            self.drivers
                .get(d)
                .is_some_and(|r| r.driver.purpose == Purpose::Work && r.state.is_live())
        })
    }

    /// The task `agent` took up last among those it has in progress.
    pub fn current_task(&self, agent: AgentId) -> Option<&TaskRecord> {
        let id = self.agents.get(&agent)?.open_tasks.last()?;
        self.tasks.get(id)
    }

    pub fn row(&self, agent: AgentId) -> Option<TeamRow<'_>> {
        let team = &self.project.as_ref()?.team;
        let task = self.current_task(agent);
        Some(TeamRow {
            agent: self.agents.get(&agent)?,
            position: team.position(agent)?,
            parent: team.parent(agent),
            effort: team.effort(agent)?,
            task,
            progress: task.and_then(|t| t.progress.as_ref()?.done),
            driver: self.acting_driver(agent).and_then(|d| self.drivers.get(&d)),
        })
    }

    /// The agents `agent` runs, retired ones left out.
    pub fn children(&self, agent: AgentId) -> Vec<AgentId> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        project
            .team
            .children(agent)
            .filter(|c| !project.team.is_retired(*c))
            .collect()
    }

    /// The trail from the CEO down to `agent`, both ends included.
    pub fn trail(&self, agent: AgentId) -> Vec<AgentId> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let mut trail = vec![agent];
        while let Some(parent) = trail.last().and_then(|a| project.team.parent(*a)) {
            if trail.contains(&parent) {
                break;
            }
            trail.push(parent);
        }
        trail.reverse();
        trail
    }

    /// The drivers list: every live driver in the project, warm ones included.
    pub fn live_drivers(&self) -> impl Iterator<Item = &DriverRecord> {
        self.drivers.values().filter(|d| d.state.is_live())
    }

    /// The jobs `agent` scheduled that can still fire, which the builder may cancel from its view.
    pub fn jobs_of(&self, agent: AgentId) -> impl Iterator<Item = &JobRecord<ProjectJob>> {
        self.jobs.values().filter(move |j| {
            matches!(j.state, JobState::Scheduled { .. })
                && match &j.job.does {
                    ProjectJob::Wake { scheduled_by, .. }
                    | ProjectJob::Delegate { scheduled_by, .. } => *scheduled_by == agent,
                    ProjectJob::Routine(_) => false,
                }
        })
    }

    /// The trials about `agent`'s skills or memories: its own skills, the shared skills it has
    /// loaded, and compactions of its memory. Whether each changed it is in its state.
    pub fn trials_about(&self, agent: AgentId) -> impl Iterator<Item = &TrialRecord> {
        let loaded: Vec<_> = self
            .agents
            .get(&agent)
            .map(|a| a.skills.iter().map(|l| l.skill.clone()).collect())
            .unwrap_or_default();
        self.trials.values().filter(move |t| match &t.trial.change {
            TrialChange::EditSkill { skill, .. } | TrialChange::NewSkill { skill, .. } => {
                skill.scope == SkillScope::Agent(agent) || loaded.contains(skill)
            }
            TrialChange::CompactMemory { agent: a, .. } => *a == agent,
        })
    }

    /// Whether the Tasks tab puts a live dot on a task: it is in progress and its assignee is at
    /// work.
    pub fn task_is_live(&self, task: TaskId) -> bool {
        match self.tasks.get(&task).map(|t| &t.state) {
            Some(TaskState::InProgress(a)) => self
                .agents
                .get(&a.assignee)
                .is_some_and(|r| r.state == AgentState::Working),
            _ => false,
        }
    }

    /// Questions the CEO asked the builder and is waiting on, with no answer yet.
    pub fn open_questions(&self) -> impl Iterator<Item = &MessageRecord> {
        self.messages
            .values()
            .filter(|m| m.message.route == Route::CeoToBuilder && m.awaited && m.reply.is_none())
    }

    /// Actions waiting for the builder's Approve or Deny.
    pub fn held_actions(&self) -> impl Iterator<Item = &ActionRecord> {
        self.actions
            .values()
            .filter(|a| a.state == ActionState::Held)
    }

    /// The sidebar's live indicator: some agent is at work.
    pub fn is_live(&self) -> bool {
        self.agents.values().any(|a| a.state == AgentState::Working)
    }

    /// The sidebar's agent count: the team without its retired members.
    pub fn agent_count(&self) -> usize {
        self.agents
            .values()
            .filter(|a| a.state != AgentState::Retired)
            .count()
    }

    pub fn budget(&self) -> Option<BudgetView> {
        let project = self.project.as_ref()?;
        let budgets = Budgets::in_force(&project.settings, &project.team);
        let line = |payer: Payer, budget: Money| BudgetLine {
            spent: self.spend.of(payer),
            budget,
            stopped: budgets.admits(&self.spend, payer).is_err(),
        };
        let leads = project
            .team
            .members()
            .filter_map(|(id, p)| match p {
                Position::Lead { budget, .. } => Some((id, line(Payer::Lead(id), *budget))),
                _ => None,
            })
            .collect();
        Some(BudgetView {
            project: line(Payer::Project, budgets.project),
            lab: line(Payer::Lab, budgets.lab),
            leads,
            agents: self.agents.values().map(|a| (a.id, a.spent)).collect(),
            usage: self.usage,
        })
    }
}
