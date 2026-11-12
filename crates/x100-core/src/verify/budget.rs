//! "No turn starts once the budget that pays for it is reached, so spend passes a budget by at most
//! the turns already running", and the charge part of "nothing a lab driver does reaches the
//! project": no charge outside the lab's cap.
//!
//! Three drivers run turns at once: the CEO's, paid by the project; a worker's under a lead, paid
//! by the lead; and a lab driver, paid by the lab. Each turn's cost is only known when it ends. A
//! turn may start only when the real driver machine and the real budget check both allow it.

use std::collections::BTreeMap;

use stateright::{Model, Property};

use super::{prove, refute};
use crate::driver::{self, DriverEvent, DriverState};
use crate::id::{AgentId, InboxItemId};
use crate::ledger::{BudgetReached, Budgets, Payer, Spend};
use crate::money::Money;

type Admits = fn(&Budgets, &Spend, Payer) -> Result<(), BudgetReached>;

const PAYERS: [Payer; 3] = [
    Payer::Project,
    Payer::Lead(AgentId::from_u128(1)),
    Payer::Lab,
];
const COSTS: [u64; 2] = [1, 3];
const MAX_COST: u64 = 3;
const TURNS_EACH: u8 = 4;

fn budgets() -> Budgets {
    // The lead's budget is larger than the project's, so only the project's total can stop its
    // workers.
    Budgets {
        project: Money::from_micros(6),
        lab: Money::from_micros(2),
        leads: BTreeMap::from([(AgentId::from_u128(1), Money::from_micros(8))]),
    }
}

struct Spending {
    admits: Admits,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct State {
    drivers: [DriverState; 3],
    turns: [u8; 3],
    spent: Spend,
    started_past_budget: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Start(usize),
    End(usize, u64),
}

fn reached(b: &Budgets, spent: &Spend, payer: Payer) -> bool {
    spent.project >= b.project || (payer != Payer::Project && spent.of(payer) >= b.of(payer))
}

impl Model for Spending {
    type State = State;
    type Action = Step;

    fn init_states(&self) -> Vec<State> {
        vec![State {
            drivers: [DriverState::Warm, DriverState::Warm, DriverState::Warm],
            turns: [0; 3],
            spent: Spend::default(),
            started_past_budget: false,
        }]
    }

    fn actions(&self, s: &State, actions: &mut Vec<Step>) {
        for i in 0..PAYERS.len() {
            match s.drivers[i] {
                DriverState::Warm if s.turns[i] < TURNS_EACH => actions.push(Step::Start(i)),
                DriverState::Running => actions.extend(COSTS.iter().map(|c| Step::End(i, *c))),
                _ => {}
            }
        }
    }

    fn next_state(&self, s: &State, step: Step) -> Option<State> {
        let mut s = s.clone();
        let b = budgets();
        match step {
            Step::Start(i) => {
                (self.admits)(&b, &s.spent, PAYERS[i]).ok()?;
                s.started_past_budget |= reached(&b, &s.spent, PAYERS[i]);
                s.drivers[i] = driver::next(
                    &s.drivers[i],
                    &DriverEvent::TurnStarted {
                        given: vec![InboxItemId::from_u128(1)],
                    },
                )
                .ok()?;
                s.turns[i] += 1;
            }
            Step::End(i, cost) => {
                s.drivers[i] = driver::next(&s.drivers[i], &DriverEvent::TurnEnded).ok()?;
                s.spent.charge(PAYERS[i], Money::from_micros(cost));
            }
        }
        Some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always(
                "no turn starts once the budget that pays for it is reached",
                |_, s: &State| !s.started_past_budget,
            ),
            Property::always(
                "spend passes the project's total by at most the turns already running",
                |_, s: &State| {
                    // Below the total before the last turn started, plus every turn that can run at
                    // once.
                    s.spent.project.micros()
                        < budgets().project.micros() + MAX_COST * PAYERS.len() as u64
                },
            ),
            Property::always(
                "no lab charge passes the lab's cap by more than one turn",
                |_, s: &State| s.spent.lab.micros() < budgets().lab.micros() + MAX_COST,
            ),
            Property::sometimes("the project reaches its total", |_, s: &State| {
                s.spent.project >= budgets().project
            }),
            Property::sometimes("the lab reaches its cap", |_, s: &State| {
                s.spent.lab >= budgets().lab
            }),
        ]
    }
}

/// Bug: a lead's workers are checked against the lead's budget only, not the project's total.
fn checks_lead_only(b: &Budgets, spent: &Spend, payer: Payer) -> Result<(), BudgetReached> {
    match payer {
        Payer::Lead(lead) if spent.of(payer) >= b.of(payer) => Err(BudgetReached::Lead(lead)),
        Payer::Lead(_) => Ok(()),
        _ => b.admits(spent, payer),
    }
}

#[test]
fn no_turn_starts_past_its_budget() {
    prove(
        "budget",
        Spending {
            admits: Budgets::admits,
        },
    );
}

#[test]
fn a_check_that_forgets_the_project_total_is_caught() {
    refute(
        "budget with bug checks_lead_only",
        Spending {
            admits: checks_lead_only,
        },
        "no turn starts once the budget that pays for it is reached",
    );
}
