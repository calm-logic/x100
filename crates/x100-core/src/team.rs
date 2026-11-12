//! The shape of a project's team: one CEO at the root, leads under it, workers under leads and
//! workers.
//!
//! [`Team::apply`] is the one check on every change to that shape, so the tree keeps one root and
//! every other agent keeps exactly one parent that is on the team.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent::{AgentEvent, Position};
use crate::effort::EffortRange;
use crate::event::ProjectEvent;
use crate::id::AgentId;
use crate::ledger::Payer;
use crate::money::Money;

/// A lead as `form_team` or a team change names it, before it has an id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LeadSpec {
    pub role: String,
    pub charter: String,
    pub effort: EffortRange,
    pub budget: Money,
}

/// A change to the team after the first, as reflection proposes it. Each is a held action in v1.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamChange {
    AddLead(LeadSpec),
    /// Move a lead's workers under another lead, then retire it.
    Fold {
        lead: AgentId,
        into: AgentId,
    },
    Promote {
        worker: AgentId,
        effort: EffortRange,
        budget: Money,
    },
    Retire {
        agent: AgentId,
    },
}

/// One change to the team's shape, as the log records it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TeamOp {
    Join {
        agent: AgentId,
        position: Position,
    },
    Move {
        agent: AgentId,
        under: AgentId,
    },
    Promote {
        agent: AgentId,
        effort: EffortRange,
        budget: Money,
    },
    Retire {
        agent: AgentId,
    },
    SetEffort {
        agent: AgentId,
        range: EffortRange,
    },
    SetBudget {
        agent: AgentId,
        budget: Money,
    },
}

impl TeamOp {
    /// The change to the team's shape that `event` records, if it records one.
    pub fn from_event(event: &ProjectEvent) -> Option<TeamOp> {
        match event {
            ProjectEvent::AgentAdded(a) => Some(TeamOp::Join {
                agent: a.id,
                position: a.position.clone(),
            }),
            ProjectEvent::Agent { agent, event } => {
                let agent = *agent;
                match event {
                    AgentEvent::Moved { under } => Some(TeamOp::Move {
                        agent,
                        under: *under,
                    }),
                    AgentEvent::Promoted { effort, budget } => Some(TeamOp::Promote {
                        agent,
                        effort: *effort,
                        budget: *budget,
                    }),
                    AgentEvent::Retired => Some(TeamOp::Retire { agent }),
                    AgentEvent::EffortSet(range) => Some(TeamOp::SetEffort {
                        agent,
                        range: *range,
                    }),
                    AgentEvent::BudgetSet(budget) => Some(TeamOp::SetBudget {
                        agent,
                        budget: *budget,
                    }),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum TeamError {
    #[error("{0} is not on the team")]
    Unknown(AgentId),
    #[error("{0} is already on the team")]
    AlreadyOnTeam(AgentId),
    #[error("a project has one CEO")]
    SecondCeo,
    #[error("{0} is retired")]
    Retired(AgentId),
    #[error("{0} is not a worker")]
    NotAWorker(AgentId),
    #[error("{0} is not a lead")]
    NotALead(AgentId),
    #[error("moving {agent} under {under} would put it below itself")]
    Cycle { agent: AgentId, under: AgentId },
    #[error("{0} still has agents under it")]
    HasChildren(AgentId),
    #[error("the CEO cannot retire")]
    CeoRetires,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Member {
    pub(crate) position: Position,
    pub(crate) retired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Team {
    pub(crate) ceo: AgentId,
    pub(crate) members: BTreeMap<AgentId, Member>,
}

impl Team {
    /// A team of one: the CEO, created with its project.
    pub fn new(ceo: AgentId, effort: EffortRange) -> Team {
        let ceo_member = Member {
            position: Position::Ceo { effort },
            retired: false,
        };
        Team {
            ceo,
            members: BTreeMap::from([(ceo, ceo_member)]),
        }
    }

    pub fn ceo(&self) -> AgentId {
        self.ceo
    }

    pub fn members(&self) -> impl Iterator<Item = (AgentId, &Position)> + '_ {
        self.members.iter().map(|(id, m)| (*id, &m.position))
    }

    pub fn position(&self, agent: AgentId) -> Option<&Position> {
        self.members.get(&agent).map(|m| &m.position)
    }

    pub fn is_retired(&self, agent: AgentId) -> bool {
        self.members.get(&agent).is_some_and(|m| m.retired)
    }

    pub fn parent(&self, agent: AgentId) -> Option<AgentId> {
        match self.position(agent)? {
            Position::Ceo { .. } => None,
            Position::Lead { .. } => Some(self.ceo),
            Position::Worker { parent, .. } => Some(*parent),
        }
    }

    pub fn children(&self, agent: AgentId) -> impl Iterator<Item = AgentId> + '_ {
        self.members
            .keys()
            .copied()
            .filter(move |a| self.parent(*a) == Some(agent))
    }

    /// The effort range in force for `agent`: its own, or for a worker without one, its parent's.
    pub fn effort(&self, agent: AgentId) -> Option<EffortRange> {
        let mut at = agent;
        for _ in 0..self.members.len() {
            match self.position(at)? {
                Position::Ceo { effort } | Position::Lead { effort, .. } => return Some(*effort),
                Position::Worker {
                    effort: Some(effort),
                    ..
                } => return Some(*effort),
                Position::Worker {
                    parent,
                    effort: None,
                } => at = *parent,
            }
        }
        None
    }

    /// Who pays for `agent`'s drivers: the lead at the top of its branch, or the project for the
    /// CEO and for a worker the CEO runs with no lead above it.
    pub fn payer(&self, agent: AgentId) -> Option<Payer> {
        let mut at = agent;
        for _ in 0..self.members.len() {
            match self.position(at)? {
                Position::Ceo { .. } => return Some(Payer::Project),
                Position::Lead { .. } => return Some(Payer::Lead(at)),
                Position::Worker { parent, .. } if *parent == self.ceo => {
                    return Some(Payer::Project);
                }
                Position::Worker { parent, .. } => at = *parent,
            }
        }
        None
    }

    /// Whether `agent` is `ancestor` or somewhere below it.
    pub fn is_within(&self, agent: AgentId, ancestor: AgentId) -> bool {
        let mut at = Some(agent);
        for _ in 0..=self.members.len() {
            match at {
                Some(a) if a == ancestor => return true,
                Some(a) => at = self.parent(a),
                None => return false,
            }
        }
        false
    }

    fn live(&self, agent: AgentId) -> Result<&Member, TeamError> {
        let m = self.members.get(&agent).ok_or(TeamError::Unknown(agent))?;
        if m.retired {
            Err(TeamError::Retired(agent))
        } else {
            Ok(m)
        }
    }

    pub fn apply(&self, op: &TeamOp) -> Result<Team, TeamError> {
        let mut team = self.clone();
        match op {
            TeamOp::Join { agent, position } => {
                if self.members.contains_key(agent) {
                    return Err(TeamError::AlreadyOnTeam(*agent));
                }
                match position {
                    Position::Ceo { .. } => return Err(TeamError::SecondCeo),
                    Position::Lead { .. } => {}
                    Position::Worker { parent, .. } => {
                        self.live(*parent)?;
                    }
                }
                team.members.insert(
                    *agent,
                    Member {
                        position: position.clone(),
                        retired: false,
                    },
                );
            }
            TeamOp::Move { agent, under } => {
                let Position::Worker { effort, .. } = self.live(*agent)?.position else {
                    return Err(TeamError::NotAWorker(*agent));
                };
                self.live(*under)?;
                if self.is_within(*under, *agent) {
                    return Err(TeamError::Cycle {
                        agent: *agent,
                        under: *under,
                    });
                }
                team.set_position(
                    *agent,
                    Position::Worker {
                        parent: *under,
                        effort,
                    },
                );
            }
            TeamOp::Promote {
                agent,
                effort,
                budget,
            } => {
                if !matches!(self.live(*agent)?.position, Position::Worker { .. }) {
                    return Err(TeamError::NotAWorker(*agent));
                }
                team.set_position(
                    *agent,
                    Position::Lead {
                        effort: *effort,
                        budget: *budget,
                    },
                );
            }
            TeamOp::Retire { agent } => {
                if *agent == self.ceo {
                    return Err(TeamError::CeoRetires);
                }
                self.live(*agent)?;
                if self.children(*agent).any(|c| !self.is_retired(c)) {
                    return Err(TeamError::HasChildren(*agent));
                }
                if let Some(m) = team.members.get_mut(agent) {
                    m.retired = true;
                }
            }
            TeamOp::SetEffort { agent, range } => {
                let position = match self.live(*agent)?.position.clone() {
                    Position::Ceo { .. } => Position::Ceo { effort: *range },
                    Position::Lead { budget, .. } => Position::Lead {
                        effort: *range,
                        budget,
                    },
                    Position::Worker { parent, .. } => Position::Worker {
                        parent,
                        effort: Some(*range),
                    },
                };
                team.set_position(*agent, position);
            }
            TeamOp::SetBudget { agent, budget } => {
                let Position::Lead { effort, .. } = self.live(*agent)?.position else {
                    return Err(TeamError::NotALead(*agent));
                };
                team.set_position(
                    *agent,
                    Position::Lead {
                        effort,
                        budget: *budget,
                    },
                );
            }
        }
        Ok(team)
    }

    fn set_position(&mut self, agent: AgentId, position: Position) {
        if let Some(m) = self.members.get_mut(&agent) {
            m.position = position;
        }
    }
}
