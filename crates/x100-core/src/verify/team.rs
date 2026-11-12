//! "An agent has exactly one parent and the tree has one root", over every sequence of team changes
//! among a CEO and three other agents.

use stateright::{Model, Property};

use super::{prove, refute};
use crate::agent::Position;
use crate::effort::EffortRange;
use crate::id::AgentId;
use crate::money::Money;
use crate::team::{Team, TeamError, TeamOp};

type Apply = fn(&Team, &TeamOp) -> Result<Team, TeamError>;

const OTHERS: u128 = 3;

fn id(n: u128) -> AgentId {
    AgentId::from_u128(n)
}

struct Teams {
    apply: Apply,
}

impl Model for Teams {
    type State = Team;
    type Action = TeamOp;

    fn init_states(&self) -> Vec<Team> {
        vec![Team::new(id(0), EffortRange::FULL)]
    }

    fn actions(&self, _: &Team, actions: &mut Vec<TeamOp>) {
        for a in 1..=OTHERS {
            let agent = id(a);
            actions.push(TeamOp::Join {
                agent,
                position: Position::Lead {
                    effort: EffortRange::FULL,
                    budget: Money::ZERO,
                },
            });
            actions.push(TeamOp::Promote {
                agent,
                effort: EffortRange::FULL,
                budget: Money::ZERO,
            });
            actions.push(TeamOp::Retire { agent });
            for p in 0..=OTHERS {
                actions.push(TeamOp::Join {
                    agent,
                    position: Position::Worker {
                        parent: id(p),
                        effort: None,
                    },
                });
                actions.push(TeamOp::Move {
                    agent,
                    under: id(p),
                });
            }
        }
    }

    fn next_state(&self, team: &Team, op: TeamOp) -> Option<Team> {
        (self.apply)(team, &op).ok()
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("the tree has one root, the CEO", |_, t: &Team| {
                t.members()
                    .filter(|(a, _)| t.parent(*a).is_none())
                    .map(|(a, _)| a)
                    .eq([t.ceo()])
                    && t.members()
                        .filter(|(_, p)| matches!(p, Position::Ceo { .. }))
                        .count()
                        == 1
            }),
            Property::always(
                "every other agent has exactly one parent on the team",
                |_, t: &Team| {
                    t.members().all(|(a, _)| {
                        a == t.ceo() || t.parent(a).is_some_and(|p| t.position(p).is_some())
                    })
                },
            ),
            Property::always("every agent reaches the root", |_, t: &Team| {
                t.members().all(|(a, _)| t.is_within(a, t.ceo()))
            }),
            Property::always(
                "no working agent sits under a retired one",
                |_, t: &Team| {
                    t.members().all(|(a, _)| {
                        t.is_retired(a) || t.parent(a).is_none_or(|p| !t.is_retired(p))
                    })
                },
            ),
            Property::sometimes("a worker three levels down", |_, t: &Team| {
                t.members().any(|(a, _)| {
                    t.parent(a)
                        .and_then(|p| t.parent(p))
                        .and_then(|g| t.parent(g))
                        .is_some()
                })
            }),
        ]
    }
}

/// Bug: moving a worker skips the check that it is not being put below itself.
fn move_skips_cycle_check(team: &Team, op: &TeamOp) -> Result<Team, TeamError> {
    match op {
        TeamOp::Move { agent, under } if team.members.contains_key(under) => {
            let mut t = team.clone();
            match t.members.get_mut(agent) {
                Some(m) if matches!(m.position, Position::Worker { .. }) => {
                    m.position = Position::Worker {
                        parent: *under,
                        effort: None,
                    };
                    Ok(t)
                }
                _ => team.apply(op),
            }
        }
        _ => team.apply(op),
    }
}

#[test]
fn the_team_keeps_one_root_and_one_parent_each() {
    prove("team", Teams { apply: Team::apply });
}

#[test]
fn a_move_that_skips_the_cycle_check_is_caught() {
    refute(
        "team with bug move_skips_cycle_check",
        Teams {
            apply: move_skips_cycle_check,
        },
        "every agent reaches the root",
    );
}
