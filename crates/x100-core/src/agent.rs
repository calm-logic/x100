//! Agents: members of a project's team. An agent is a record, not a model and not a process. It
//! keeps its identity and its work while drivers come and go.

use serde::{Deserialize, Serialize};

use crate::driver::Incompatible;
use crate::effort::EffortRange;
use crate::id::{AgentId, InboxItemId, WaitId};
use crate::inbox::InboxItem;
use crate::machine::{MachineKind, Rejected};
use crate::money::Money;
use crate::wait::{Wait, WaitEnd};

/// An agent as it joins the team.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    /// A short name such as "Tech" or "QA". The CEO may invent any role.
    pub role: String,
    pub charter: String,
    pub position: Position,
}

/// Where an agent sits in the tree. The CEO has no parent, a lead's parent is the CEO, and a worker
/// names its parent, so every agent but the CEO has exactly one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Ceo {
        effort: EffortRange,
    },
    Lead {
        effort: EffortRange,
        budget: Money,
    },
    Worker {
        parent: AgentId,
        /// `None` means the parent's range, whatever it is at the time.
        effort: Option<EffortRange>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Named in the team, and no work has reached it yet. A lead is not started until there is work
    /// for it.
    Planned,
    Working,
    Waiting(Waiting),
    /// No open task and nothing to wait for.
    Idle,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Waiting {
    /// It called `wait`. Its waits are its open business.
    OnWaits,
    /// No compatible driver could be found, for these reasons.
    ForDriver(Vec<Incompatible>),
}

impl AgentState {
    pub const INITIAL: AgentState = AgentState::Planned;

    pub fn name(&self) -> &'static str {
        match self {
            AgentState::Planned => "Planned",
            AgentState::Working => "Working",
            AgentState::Waiting(Waiting::OnWaits) => "Waiting(OnWaits)",
            AgentState::Waiting(Waiting::ForDriver(_)) => "Waiting(ForDriver)",
            AgentState::Idle => "Idle",
            AgentState::Retired => "Retired",
        }
    }
}

/// Everything that happens to one agent. Inbox deliveries and waits are here because they are what
/// move the agent between states.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    /// Something reached its inbox. Anything new wakes a waiting or idle agent.
    Delivered {
        item: InboxItemId,
        what: InboxItem,
    },
    /// It called `wait`.
    WaitOpened(Wait),
    /// A wait ended. Waking is done by the inbox item that says so, not by this.
    WaitEnded {
        wait: WaitId,
        end: WaitEnd,
    },
    NoDriver(Vec<Incompatible>),
    DriverFound,
    WentIdle,
    Retired,
    EffortSet(EffortRange),
    BudgetSet(Money),
    /// A worker moved under another agent, as when a lead is folded into another.
    Moved {
        under: AgentId,
    },
    /// A worker made a lead.
    Promoted {
        effort: EffortRange,
        budget: Money,
    },
}

impl AgentEvent {
    pub fn name(&self) -> &'static str {
        match self {
            AgentEvent::Delivered { .. } => "Delivered",
            AgentEvent::WaitOpened(_) => "WaitOpened",
            AgentEvent::WaitEnded { .. } => "WaitEnded",
            AgentEvent::NoDriver(_) => "NoDriver",
            AgentEvent::DriverFound => "DriverFound",
            AgentEvent::WentIdle => "WentIdle",
            AgentEvent::Retired => "Retired",
            AgentEvent::EffortSet(_) => "EffortSet",
            AgentEvent::BudgetSet(_) => "BudgetSet",
            AgentEvent::Moved { .. } => "Moved",
            AgentEvent::Promoted { .. } => "Promoted",
        }
    }
}

/// The agent machine. Planned → Working when its first inbox item arrives. Working → Waiting when
/// it waits, or when no driver can be found; Waiting → Working when anything reaches its inbox, or,
/// waiting for a driver, when one is found. Working → Idle; Idle → Working when anything reaches
/// its inbox. Waiting for a driver → Idle when that wait reached its deadline and the task was
/// reported failed. Idle → Retired, and Planned → Retired for a lead a team change removes before
/// any work reached it. There is no Failed state: drivers fail, agents do not.
///
/// Changes to the agent's place in the team leave its state as it is, and are refused once it is
/// retired. Whether they are allowed by the team's shape is [`crate::team::Team`]'s question.
pub fn next(state: &AgentState, event: &AgentEvent) -> Result<AgentState, Rejected> {
    use AgentEvent as E;
    use AgentState as S;
    let next = match (state, event) {
        (S::Retired, _) => None,
        (_, E::Delivered { .. }) => Some(S::Working),
        (S::Working, E::WaitOpened(_)) => Some(S::Waiting(Waiting::OnWaits)),
        (S::Working | S::Waiting(_), E::WaitEnded { .. }) => Some(state.clone()),
        (S::Working, E::NoDriver(why)) => Some(S::Waiting(Waiting::ForDriver(why.clone()))),
        (S::Waiting(Waiting::ForDriver(_)), E::DriverFound) => Some(S::Working),
        (S::Working | S::Waiting(Waiting::ForDriver(_)), E::WentIdle) => Some(S::Idle),
        (S::Planned | S::Idle, E::Retired) => Some(S::Retired),
        (_, E::EffortSet(_) | E::BudgetSet(_) | E::Moved { .. } | E::Promoted { .. }) => {
            Some(state.clone())
        }
        _ => None,
    };
    next.ok_or(Rejected {
        machine: MachineKind::Agent,
        state: state.name(),
        event: event.name(),
    })
}
