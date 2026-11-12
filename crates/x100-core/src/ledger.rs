//! The ledger: who pays for what, and the rule that stops work at a budget.
//!
//! The project has a total. Each lead has a budget inside it, and the lab has a cap inside it. A
//! lead's workers spend from that lead's budget. The CEO, and any worker the CEO runs with no lead
//! above it, spend from the project directly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{AgentId, DriverId, OneOffCallId};
use crate::money::{Money, Usage};

/// The budget line a charge is made against. Every line also counts against the project's total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Payer {
    Project,
    Lead(AgentId),
    Lab,
}

/// One entry in the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Charge {
    pub payer: Payer,
    pub source: ChargeSource,
    pub usage: Usage,
    /// What the harness reported, or the tokens at the model's listed price.
    pub cost: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeSource {
    Turn { driver: DriverId, agent: AgentId },
    OneOff { call: OneOffCallId },
}

/// Which budget stopped a turn from starting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum BudgetReached {
    #[error("the project has reached its budget")]
    Project,
    #[error("lead {0} has reached its budget")]
    Lead(AgentId),
    #[error("the lab has reached its cap")]
    Lab,
}

/// The limits in force: the project's total, each lead's budget and the lab's cap.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Budgets {
    pub project: Money,
    pub lab: Money,
    pub leads: BTreeMap<AgentId, Money>,
}

/// What has been spent, on the same lines as [`Budgets`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Spend {
    pub project: Money,
    pub lab: Money,
    pub leads: BTreeMap<AgentId, Money>,
}

impl Spend {
    pub fn charge(&mut self, payer: Payer, cost: Money) {
        self.project += cost;
        match payer {
            Payer::Project => {}
            Payer::Lead(lead) => *self.leads.entry(lead).or_default() += cost,
            Payer::Lab => self.lab += cost,
        }
    }

    pub fn of(&self, payer: Payer) -> Money {
        match payer {
            Payer::Project => self.project,
            Payer::Lead(lead) => self.leads.get(&lead).copied().unwrap_or_default(),
            Payer::Lab => self.lab,
        }
    }
}

impl Budgets {
    pub fn of(&self, payer: Payer) -> Money {
        match payer {
            Payer::Project => self.project,
            Payer::Lead(lead) => self.leads.get(&lead).copied().unwrap_or_default(),
            Payer::Lab => self.lab,
        }
    }

    /// Whether a turn paid by `payer` may start. A budget is reached once spend meets it, and a
    /// reached budget stops the next turn, a further turn on a warm driver included. Cost is only
    /// known after a turn, so spend can pass a budget by the turns already running when it was
    /// reached, and by no more.
    pub fn admits(&self, spent: &Spend, payer: Payer) -> Result<(), BudgetReached> {
        if spent.project >= self.project {
            return Err(BudgetReached::Project);
        }
        let reached = match payer {
            Payer::Project => return Ok(()),
            Payer::Lead(lead) => BudgetReached::Lead(lead),
            Payer::Lab => BudgetReached::Lab,
        };
        if spent.of(payer) >= self.of(payer) {
            Err(reached)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lead() -> AgentId {
        AgentId::from_u128(1)
    }

    fn budgets() -> Budgets {
        Budgets {
            project: Money::from_micros(100),
            lab: Money::from_micros(10),
            leads: BTreeMap::from([(lead(), Money::from_micros(40))]),
        }
    }

    #[test]
    fn every_line_counts_against_the_project() {
        let mut spent = Spend::default();
        spent.charge(Payer::Lead(lead()), Money::from_micros(5));
        spent.charge(Payer::Lab, Money::from_micros(3));
        spent.charge(Payer::Project, Money::from_micros(2));
        assert_eq!(spent.of(Payer::Project), Money::from_micros(10));
        assert_eq!(spent.of(Payer::Lead(lead())), Money::from_micros(5));
        assert_eq!(spent.of(Payer::Lab), Money::from_micros(3));
    }

    #[test]
    fn a_lead_at_its_budget_starts_nothing_while_others_carry_on() {
        let mut spent = Spend::default();
        spent.charge(Payer::Lead(lead()), Money::from_micros(40));
        assert_eq!(
            budgets().admits(&spent, Payer::Lead(lead())),
            Err(BudgetReached::Lead(lead()))
        );
        assert_eq!(budgets().admits(&spent, Payer::Project), Ok(()));
        let other = AgentId::from_u128(2);
        let mut b = budgets();
        b.leads.insert(other, Money::from_micros(40));
        assert_eq!(b.admits(&spent, Payer::Lead(other)), Ok(()));
    }

    #[test]
    fn a_project_at_its_total_starts_nothing() {
        let mut spent = Spend::default();
        spent.charge(Payer::Project, Money::from_micros(100));
        for payer in [Payer::Project, Payer::Lead(lead()), Payer::Lab] {
            assert_eq!(budgets().admits(&spent, payer), Err(BudgetReached::Project));
        }
    }

    #[test]
    fn the_lab_stops_at_its_cap() {
        let mut spent = Spend::default();
        spent.charge(Payer::Lab, Money::from_micros(10));
        assert_eq!(
            budgets().admits(&spent, Payer::Lab),
            Err(BudgetReached::Lab)
        );
        assert_eq!(budgets().admits(&spent, Payer::Project), Ok(()));
    }

    #[test]
    fn a_lead_with_no_budget_set_starts_nothing() {
        let unknown = AgentId::from_u128(9);
        assert_eq!(
            budgets().admits(&Spend::default(), Payer::Lead(unknown)),
            Err(BudgetReached::Lead(unknown))
        );
    }
}
