//! Charging a turn: who pays, what it cost and on what basis, and which budgets it reaches.
//!
//! The charge goes to the lead at the top of the agent's branch, to the project for the CEO and
//! for a worker the CEO runs directly, and to the lab line for a lab driver. Whether a turn may
//! start is core's rule, [`x100_core::ledger::Budgets::admits`]; this says which lines a charge
//! has just taken to their limit, so the daemon can tell whoever must hear of it once.

use x100_core::driver::{Driver, Purpose};
use x100_core::fold::ProjectState;
use x100_core::ledger::{BudgetReached, Budgets, Charge, ChargeSource, CostBasis, Payer, Spend};
use x100_core::money::{Money, Usage};

/// A charge, and the budgets it took to their limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Billed {
    pub charge: Charge,
    pub reached: Vec<BudgetReached>,
}

/// Charges one turn of `driver`. `cost` and `basis` are what its harness said: a cost it reported,
/// or the tokens it reported at the model's listed price. `None` for an agent no longer on the
/// team, whom nobody pays for.
pub fn bill(
    project: &ProjectState,
    driver: &Driver,
    usage: Usage,
    cost: Money,
    basis: CostBasis,
) -> Option<Billed> {
    let p = project.project()?;
    let payer = match driver.purpose {
        Purpose::Work => p.team.payer(driver.agent)?,
        Purpose::Lab { .. } => Payer::Lab,
    };
    let budgets = Budgets::in_force(&p.settings, &p.team);
    let view = project.budget()?;
    let mut spend = Spend {
        project: view.project.spent,
        lab: view.lab.spent,
        leads: view.leads.iter().map(|(id, l)| (*id, l.spent)).collect(),
    };
    let before = reached(&budgets, &spend, payer);
    spend.charge(payer, cost);
    let after = reached(&budgets, &spend, payer);
    Some(Billed {
        charge: Charge {
            payer,
            source: ChargeSource::Turn {
                driver: driver.id,
                agent: driver.agent,
            },
            usage,
            cost,
            basis,
        },
        reached: after.into_iter().filter(|r| !before.contains(r)).collect(),
    })
}

/// The lines that pay for a turn of `payer` and are at their limit: the project's total, and the
/// payer's own budget.
fn reached(budgets: &Budgets, spend: &Spend, payer: Payer) -> Vec<BudgetReached> {
    let mut out = Vec::new();
    if spend.project >= budgets.project {
        out.push(BudgetReached::Project);
    }
    let line = match payer {
        Payer::Project => None,
        Payer::Lead(lead) => Some(BudgetReached::Lead(lead)),
        Payer::Lab => Some(BudgetReached::Lab),
    };
    if let Some(line) = line
        && spend.of(payer) >= budgets.of(payer)
    {
        out.push(line);
    }
    out
}
