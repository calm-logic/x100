//! The team's shape: one root, one parent each, and who pays.

use proptest::prelude::*;
use x100_core::action::AskMeAbout;
use x100_core::agent::Position;
use x100_core::effort::{Effort, EffortRange};
use x100_core::id::AgentId;
use x100_core::ledger::{Budgets, LeadsOverTotal, Payer};
use x100_core::money::{Money, Percent};
use x100_core::project::Settings;
use x100_core::team::{Team, TeamError, TeamOp};

fn id(n: u128) -> AgentId {
    AgentId::from_u128(n)
}

fn range(min: Effort, max: Effort) -> EffortRange {
    EffortRange::new(min, max).unwrap()
}

fn lead(n: u128) -> TeamOp {
    TeamOp::Join {
        agent: id(n),
        position: Position::Lead {
            effort: EffortRange::FULL,
            budget: Money::from_dollars(1),
        },
    }
}

fn worker(n: u128, parent: u128) -> TeamOp {
    TeamOp::Join {
        agent: id(n),
        position: Position::Worker {
            parent: id(parent),
            effort: None,
        },
    }
}

/// CEO 0, lead 1 with worker 2 and its worker 3, lead 4, and worker 5 run by the CEO directly.
fn team() -> Team {
    [lead(1), worker(2, 1), worker(3, 2), lead(4), worker(5, 0)]
        .iter()
        .try_fold(Team::new(id(0), EffortRange::FULL), |t, op| t.apply(op))
        .unwrap()
}

#[test]
fn costs_go_to_the_lead_at_the_top_of_the_branch() {
    let t = team();
    assert_eq!(t.payer(id(0)), Some(Payer::Project));
    assert_eq!(t.payer(id(1)), Some(Payer::Lead(id(1))));
    assert_eq!(t.payer(id(3)), Some(Payer::Lead(id(1))));
    assert_eq!(t.payer(id(5)), Some(Payer::Project));
    assert_eq!(t.payer(id(9)), None);
}

#[test]
fn a_worker_has_its_parents_range_unless_it_has_its_own() {
    let t = team()
        .apply(&TeamOp::SetEffort {
            agent: id(1),
            range: range(Effort::Low, Effort::Medium),
        })
        .unwrap();
    assert_eq!(t.effort(id(3)), Some(range(Effort::Low, Effort::Medium)));
    let t = t
        .apply(&TeamOp::SetEffort {
            agent: id(2),
            range: range(Effort::High, Effort::Max),
        })
        .unwrap();
    assert_eq!(t.effort(id(3)), Some(range(Effort::High, Effort::Max)));
}

#[test]
fn shape_changes_that_would_break_the_tree_are_refused() {
    let t = team();
    let ceo = TeamOp::Join {
        agent: id(7),
        position: Position::Ceo {
            effort: EffortRange::FULL,
        },
    };
    assert_eq!(t.apply(&ceo), Err(TeamError::SecondCeo));
    assert_eq!(t.apply(&worker(7, 9)), Err(TeamError::Unknown(id(9))));
    assert_eq!(t.apply(&lead(1)), Err(TeamError::AlreadyOnTeam(id(1))));
    assert_eq!(
        t.apply(&TeamOp::Move {
            agent: id(2),
            under: id(3)
        }),
        Err(TeamError::Cycle {
            agent: id(2),
            under: id(3)
        })
    );
    assert_eq!(
        t.apply(&TeamOp::Move {
            agent: id(2),
            under: id(2)
        }),
        Err(TeamError::Cycle {
            agent: id(2),
            under: id(2)
        })
    );
    assert_eq!(
        t.apply(&TeamOp::Move {
            agent: id(1),
            under: id(4)
        }),
        Err(TeamError::NotAWorker(id(1)))
    );
    assert_eq!(
        t.apply(&TeamOp::Retire { agent: id(1) }),
        Err(TeamError::HasChildren(id(1)))
    );
    assert_eq!(
        t.apply(&TeamOp::Retire { agent: id(0) }),
        Err(TeamError::CeoRetires)
    );
    assert_eq!(
        t.apply(&TeamOp::SetBudget {
            agent: id(2),
            budget: Money::ZERO
        }),
        Err(TeamError::NotALead(id(2)))
    );
}

#[test]
fn folding_a_lead_into_another_moves_its_workers_then_retires_it() {
    let t = team()
        .apply(&TeamOp::Move {
            agent: id(2),
            under: id(4),
        })
        .unwrap();
    let t = t.apply(&TeamOp::Retire { agent: id(1) }).unwrap();
    assert!(t.is_retired(id(1)));
    assert_eq!(t.parent(id(2)), Some(id(4)));
    assert_eq!(t.payer(id(3)), Some(Payer::Lead(id(4))));
    assert_eq!(t.apply(&worker(8, 1)), Err(TeamError::Retired(id(1))));
}

#[test]
fn a_promoted_worker_reports_to_the_ceo_and_pays_for_its_branch() {
    let t = team()
        .apply(&TeamOp::Promote {
            agent: id(2),
            effort: EffortRange::FULL,
            budget: Money::from_dollars(3),
        })
        .unwrap();
    assert_eq!(t.parent(id(2)), Some(id(0)));
    assert_eq!(t.payer(id(3)), Some(Payer::Lead(id(2))));
}

fn op(n: u128) -> impl Strategy<Value = TeamOp> {
    let a = 1..n;
    let b = 0..n;
    prop_oneof![
        a.clone().prop_map(lead),
        (a.clone(), b.clone()).prop_map(|(x, p)| worker(x, p)),
        (a.clone(), b).prop_map(|(x, u)| TeamOp::Move {
            agent: id(x),
            under: id(u)
        }),
        a.clone().prop_map(|x| TeamOp::Promote {
            agent: id(x),
            effort: EffortRange::FULL,
            budget: Money::ZERO
        }),
        a.prop_map(|x| TeamOp::Retire { agent: id(x) }),
    ]
}

proptest! {
    /// Whatever sequence of changes is tried, the ones accepted leave one root and every other
    /// agent with a parent on the team, from which the root is reached.
    #[test]
    fn accepted_changes_keep_one_root_and_one_parent_each(
        ops in prop::collection::vec(op(8), 0..40)
    ) {
        let mut t = Team::new(id(0), EffortRange::FULL);
        for o in &ops {
            if let Ok(next) = t.apply(o) {
                t = next;
            }
        }
        let members: Vec<_> = t.members().map(|(a, _)| a).collect();
        let roots: Vec<_> = members.iter().copied().filter(|a| t.parent(*a).is_none()).collect();
        prop_assert_eq!(roots, vec![id(0)]);
        for a in &members {
            prop_assert!(t.is_within(*a, id(0)), "{a} does not reach the CEO");
            if let Some(p) = t.parent(*a) {
                prop_assert!(members.contains(&p));
                if !t.is_retired(*a) {
                    prop_assert!(!t.is_retired(p), "{a} sits under retired {p}");
                }
            }
        }
    }
}

fn settings(total: u64) -> Settings {
    Settings {
        ask_me_about: AskMeAbout::HighRiskOnly,
        budget: Money::from_dollars(total),
        lab_share: Percent::new(10).unwrap(),
        allowed: Default::default(),
    }
}

fn lead_with(n: u128, dollars: u64) -> TeamOp {
    TeamOp::Join {
        agent: id(n),
        position: Position::Lead {
            effort: EffortRange::FULL,
            budget: Money::from_dollars(dollars),
        },
    }
}

fn check(total: u64, team: &Team) -> Result<(), LeadsOverTotal> {
    Budgets::in_force(&settings(total), team).check_leads()
}

#[test]
fn a_form_team_whose_budgets_pass_the_total_is_refused() {
    let base = Team::new(id(0), EffortRange::FULL);
    let fits = [lead_with(1, 20), lead_with(2, 30)]
        .iter()
        .try_fold(base.clone(), |t, op| t.apply(op))
        .unwrap();
    assert_eq!(check(50, &fits), Ok(()));
    let over = fits.apply(&lead_with(3, 1)).unwrap();
    assert_eq!(
        check(50, &over),
        Err(LeadsOverTotal {
            leads: Money::from_dollars(51),
            total: Money::from_dollars(50)
        })
    );
}

#[test]
fn set_budget_and_the_builders_changes_are_checked_against_the_total() {
    let t = [lead_with(1, 20), lead_with(2, 20)]
        .iter()
        .try_fold(Team::new(id(0), EffortRange::FULL), |t, op| t.apply(op))
        .unwrap();
    let raised = t
        .apply(&TeamOp::SetBudget {
            agent: id(1),
            budget: Money::from_dollars(31),
        })
        .unwrap();
    assert!(check(50, &raised).is_err());
    // The builder lowering the total under what the leads hold is refused the same way.
    assert!(check(39, &t).is_err());
    assert_eq!(check(40, &t), Ok(()));
}

#[test]
fn a_retired_leads_budget_no_longer_counts() {
    let t = [lead_with(1, 30), lead_with(2, 20)]
        .iter()
        .try_fold(Team::new(id(0), EffortRange::FULL), |t, op| t.apply(op))
        .unwrap();
    let t = t.apply(&TeamOp::Retire { agent: id(2) }).unwrap();
    let t = t.apply(&lead_with(3, 20)).unwrap();
    assert_eq!(check(50, &t), Ok(()));
    assert_eq!(t.lead_budgets().count(), 2);
}
