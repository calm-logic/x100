//! The five rules of "Finding a driver", as one table, and the order that makes "cheapest" mean
//! one thing.
//!
//! A driver is compatible with an agent's work when every rule passes it:
//!
//! | # | Rule | Excluded as |
//! |---|---|---|
//! | 1 | Its effort is inside the agent's range and at least what the task needs, clamped into it | `NoEffortInRange` |
//! | 2 | The builder allows its harness and model in the project | `NothingAllowed` |
//! | 3 | Its harness and model are not cooling off | `CoolingOff` |
//! | 4 | Its machine is under its cap on live drivers, and a terminal driver's login under its cap | `MachineFull` |
//! | 5 | The budget that pays for it, and the project's total, admit another turn | `Budget` |
//!
//! Cheapest compares a blended list price: what a reference turn of three input tokens to every
//! output token costs at the model's base rates for that effort level. Ties go to the lower effort,
//! then the harness id, the model name and the machine id, so the same state always gives the same
//! driver.

use x100_core::driver::Incompatible;
use x100_core::effort::{Effort, EffortRange};
use x100_core::fold::{Project, ProjectState};
use x100_core::harness::{HarnessModel, ModelName, ModelOffer};
use x100_core::id::{AgentId, HarnessId, MachineId};
use x100_core::ledger::{BudgetReached, Budgets, Payer, Spend};
use x100_core::money::Price;

use crate::world::{Login, Occupancy, TERMINAL_DRIVERS_PER_LOGIN, World};

/// A driver that could run the agent's work: a new one, or its warm one.
#[derive(Debug, Clone)]
pub(crate) struct Candidate<'a> {
    pub harness: HarnessId,
    pub model: &'a ModelName,
    pub effort: Effort,
    pub price: Option<&'a Price>,
    pub machine: MachineId,
    pub login: Option<&'a Login>,
    /// A warm driver already holds its room on the machine.
    pub holds_slot: bool,
}

impl Candidate<'_> {
    fn of(&self) -> HarnessModel {
        HarnessModel {
            harness: self.harness,
            model: self.model.clone(),
        }
    }

    /// The order "cheapest" sorts by.
    pub fn cost_key(&self) -> (u128, Effort, HarnessId, &ModelName, MachineId) {
        let price = self.price.map_or(u128::MAX, blended);
        (price, self.effort, self.harness, self.model, self.machine)
    }
}

/// The blended list price of a level, in micro-dollars per four million tokens: three million
/// input and one million output at its base rates.
pub fn blended(price: &Price) -> u128 {
    3 * u128::from(price.rates.input.micros()) + u128::from(price.rates.output.micros())
}

/// The effort `agent`'s next turn needs: its current task's, which the delegator set and the
/// daemon clamped into its range, or the bottom of its range when it has no task.
pub fn needs(project: &ProjectState, agent: AgentId, range: EffortRange) -> Effort {
    project
        .current_task(agent)
        .and_then(|t| t.state.assignment())
        .map_or(range.min(), |a| range.clamp(a.effort))
}

/// The question every rule answers: may this agent's next turn run on this candidate?
pub(crate) struct Ask<'a> {
    pub world: World<'a>,
    pub project: &'a Project,
    pub range: EffortRange,
    pub needs: Effort,
    pub payer: Payer,
    pub budgets: Budgets,
    pub spend: Spend,
    pub occupancy: Occupancy,
}

impl<'a> Ask<'a> {
    pub fn new(
        world: World<'a>,
        project: &'a Project,
        range: EffortRange,
        needs: Effort,
        payer: Payer,
    ) -> Self {
        let view = world.project.budget();
        let spend = view.map(|v| Spend {
            project: v.project.spent,
            lab: v.lab.spent,
            leads: v.leads.iter().map(|(id, l)| (*id, l.spent)).collect(),
        });
        Ask {
            world,
            project,
            range,
            needs,
            payer,
            budgets: Budgets::in_force(&project.settings, &project.team),
            spend: spend.unwrap_or_default(),
            occupancy: world.occupancy.clone(),
        }
    }

    /// Rule 5 alone, which does not depend on the driver.
    pub fn budget(&self) -> Result<(), BudgetReached> {
        self.budgets.admits(&self.spend, self.payer)
    }
}

pub(crate) type Rule = fn(&Ask, &Candidate) -> Result<(), Incompatible>;

/// The rules in the order the documents list them. A candidate's first failure is why it was
/// excluded.
pub(crate) const RULES: [Rule; 5] = [effort, allowed, cool_off, machine, budget];

pub(crate) fn effort(ask: &Ask, c: &Candidate) -> Result<(), Incompatible> {
    if ask.range.admits(c.effort, ask.needs) {
        Ok(())
    } else {
        Err(Incompatible::NoEffortInRange)
    }
}

pub(crate) fn allowed(ask: &Ask, c: &Candidate) -> Result<(), Incompatible> {
    if ask.project.settings.allowed.contains(&c.of()) {
        Ok(())
    } else {
        Err(Incompatible::NothingAllowed)
    }
}

pub(crate) fn cool_off(ask: &Ask, c: &Candidate) -> Result<(), Incompatible> {
    let of = c.of();
    match ask.world.daemon.cooling_off(&of) {
        Some(_) => Err(Incompatible::CoolingOff(of)),
        None => Ok(()),
    }
}

/// Core has no reason for a full login, so a login at its cap is reported as its machine full.
pub(crate) fn machine(ask: &Ask, c: &Candidate) -> Result<(), Incompatible> {
    if c.holds_slot {
        return Ok(());
    }
    let full = Err(Incompatible::MachineFull { machine: c.machine });
    let cap = ask
        .world
        .daemon
        .machines()
        .find(|m| m.id == c.machine)
        .map_or(0, |m| m.cap);
    if ask.occupancy.on_machine(c.machine) >= cap {
        return full;
    }
    match c.login {
        Some(login) if ask.occupancy.on_login(login) >= TERMINAL_DRIVERS_PER_LOGIN => full,
        _ => Ok(()),
    }
}

pub(crate) fn budget(ask: &Ask, _: &Candidate) -> Result<(), Incompatible> {
    ask.budget().map_err(Incompatible::Budget)
}

/// Every new driver this daemon could start: each level of each model of each harness it can
/// start, on its machine. A model none of whose levels passes rule 1 is left out, and named.
pub(crate) fn candidates<'a>(ask: &Ask<'a>) -> (Vec<Candidate<'a>>, bool) {
    let world = ask.world;
    let installed = world.installed;
    let mut out = Vec::new();
    let mut no_level = false;
    let on_machine = world.daemon.machines().any(|m| m.id == installed.machine);
    let harnesses = world
        .daemon
        .harnesses()
        .filter(|h| on_machine && installed.harnesses.contains(&h.id));
    for h in harnesses {
        for offer in &h.models {
            let before = out.len();
            out.extend(levels(ask, h.id, offer));
            no_level |= out.len() == before;
        }
    }
    out.sort_by(|a, b| a.cost_key().cmp(&b.cost_key()));
    (out, no_level)
}

fn levels<'a>(ask: &Ask<'a>, harness: HarnessId, offer: &'a ModelOffer) -> Vec<Candidate<'a>> {
    let installed = ask.world.installed;
    offer
        .levels
        .iter()
        .map(|(effort, price)| Candidate {
            harness,
            model: &offer.model,
            effort: *effort,
            price: Some(price),
            machine: installed.machine,
            login: installed.logins.get(&harness),
            holds_slot: false,
        })
        .filter(|c| effort(ask, c).is_ok())
        .collect()
}
