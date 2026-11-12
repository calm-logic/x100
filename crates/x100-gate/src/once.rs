//! "Allowed once": what an Approve of a command or an egress request leaves behind.
//!
//! A command stopped by a permission hook, or an egress request refused at once, was answered
//! while it was held. On Approve nothing is run for it: the agent tries again, and this record
//! lets exactly that attempt through, one time.
//!
//! The rules: a grant matches only the same agent, and only the exact command (or the exact host
//! and method), not a similar one and not through another driver's address. A match uses it up.
//! A grant is forgotten when its agent's task ends or the agent leaves the team.
//!
//! The record is not stored on its own. An approved command or egress action that has not been
//! carried out is a grant, so [`AllowedOnce::from_state`] rebuilds it from the folded log after a
//! restart, and using a grant is the action's `CarriedOut` event, which the caller appends.

use x100_core::action::{Action, ActionState, Request};
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId, TaskId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permit {
    Command(String),
    Egress { host: String, method: String },
}

impl Permit {
    fn of(request: &Request) -> Option<Permit> {
        match request {
            Request::Command { command, .. } => Some(Permit::Command(command.clone())),
            Request::Egress { host, method, .. } => Some(Permit::Egress {
                host: host.clone(),
                method: method.clone(),
            }),
            Request::Tool { .. } | Request::Merge { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The approved action. Using the grant carries it out.
    pub action: ActionId,
    pub agent: AgentId,
    /// The task the agent was on when the builder approved, if it had one.
    pub task: Option<TaskId>,
    pub permit: Permit,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowedOnce {
    grants: Vec<Grant>,
}

impl AllowedOnce {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the builder's Approve of `action`. Only a command or an egress request leaves a
    /// grant; a tool call is carried out by the daemon and a merge is done, so neither does.
    /// Approving the same action twice grants once.
    pub fn grant(&mut self, action: &Action, task: Option<TaskId>) -> bool {
        let Some(permit) = Permit::of(&action.request) else {
            return false;
        };
        if self.grants.iter().any(|g| g.action == action.id) {
            return false;
        }
        self.grants.push(Grant {
            action: action.id,
            agent: action.agent,
            task,
            permit,
        });
        true
    }

    /// Lets `request` through if `agent` holds a grant for exactly it, and uses the grant up.
    /// Returns the approved action, which the caller marks carried out.
    pub fn take(&mut self, agent: AgentId, request: &Request) -> Option<ActionId> {
        let want = Permit::of(request)?;
        let at = self
            .grants
            .iter()
            .position(|g| g.agent == agent && g.permit == want)?;
        Some(self.grants.remove(at).action)
    }

    /// The agent's task ended: its grants are forgotten, and so are those it got with no task.
    pub fn task_ended(&mut self, agent: AgentId, task: TaskId) {
        self.grants
            .retain(|g| g.agent != agent || g.task.is_some_and(|t| t != task));
    }

    pub fn agent_left(&mut self, agent: AgentId) {
        self.grants.retain(|g| g.agent != agent);
    }

    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }

    /// Every approved command and egress action that was not carried out, for an agent that still
    /// has a task. The grant's task is the agent's current one, which is the nearest the log
    /// comes to the one it had when the builder approved.
    pub fn from_state(state: &ProjectState) -> Self {
        let mut once = Self::new();
        for record in state.actions() {
            let agent = record.action.agent;
            let working = state.agent(agent).is_some_and(|a| !a.open_tasks.is_empty());
            if record.state == ActionState::Approved && working {
                let task = state.current_task(agent).map(|t| t.task.id);
                once.grant(&record.action, task);
            }
        }
        once
    }
}
