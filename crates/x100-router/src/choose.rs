//! The choice: the agent's warm driver if it is compatible, else the cheapest compatible new one,
//! else every reason nothing is.

use std::collections::BTreeSet;

use x100_core::driver::{Incompatible, PlannedReason, Purpose, RetireReason, View};
use x100_core::effort::Effort;
use x100_core::harness::ModelName;
use x100_core::id::{AgentId, DriverId, HarnessId, MachineId, TaskId};
use x100_core::ledger::Payer;

use crate::rules::{self, Ask, Candidate, RULES};
use crate::upkeep::worn;
use crate::world::World;

/// A driver to start: the harness, model and effort, where it runs and how it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub harness: HarnessId,
    pub model: ModelName,
    pub effort: Effort,
    pub machine: MachineId,
    pub view: View,
}

/// The agent's live driver, between turns, as the orchestrator knows it. A driver never outlives
/// the daemon, so what the orchestrator holds in memory about it is complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warm {
    pub driver: DriverId,
    /// Every task it has had a turn on.
    pub carried: BTreeSet<TaskId>,
    /// A task it carried was accepted since it last compacted.
    pub accepted: bool,
}

/// Whose turn is being decided.
#[derive(Debug, Clone)]
pub struct Need {
    pub agent: AgentId,
    pub purpose: Purpose,
    pub warm: Option<Warm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Give the turn to the agent's warm driver, compacting it first if a task it carried was
    /// accepted since it last compacted.
    Reuse { driver: DriverId, compact: bool },
    /// Start this driver, after ending the agent's warm one if there is one.
    Start {
        choice: Choice,
        ending: Option<Ending>,
    },
}

/// A warm driver that must end before its agent's next driver starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    pub driver: DriverId,
    pub why: EndWhy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndWhy {
    Retire(RetireReason),
    /// It is healthy but may not take this work. Recorded as a planned handoff with no last
    /// turn, since between turns there is nothing left for one to write.
    Unfit(PlannedReason),
}

/// Decides `need`'s next turn. `Err` lists, rule by rule, every reason that excluded a driver, so
/// whoever is told can see what would help.
pub fn choose(world: &World, need: &Need) -> Result<Decision, Vec<Incompatible>> {
    let project = world
        .project
        .project()
        .ok_or_else(|| vec![Incompatible::NothingAllowed])?;
    let team = &project.team;
    let range = team
        .effort(need.agent)
        .ok_or_else(|| vec![Incompatible::NoEffortInRange])?;
    let payer = match need.purpose {
        Purpose::Work => team.payer(need.agent),
        Purpose::Lab { .. } => Some(Payer::Lab),
    }
    .ok_or_else(|| vec![Incompatible::NothingAllowed])?;
    let ask = Ask::new(
        *world,
        project,
        range,
        rules::needs(world.project, need.agent, range),
        payer,
    );
    let mut ending = None;
    if let Some(warm) = &need.warm {
        match keep(&ask, need.agent, warm) {
            Ok(compact) => {
                return ask
                    .budget()
                    .map(|()| Decision::Reuse {
                        driver: warm.driver,
                        compact,
                    })
                    .map_err(|r| vec![Incompatible::Budget(r)]);
            }
            Err(why) => {
                ending = Some(Ending {
                    driver: warm.driver,
                    why,
                })
            }
        }
    }
    allocate(ask, ending)
}

/// Whether the warm driver may take the next turn: rules 1 to 3 (it holds its room, and the
/// budget is the same for any driver), and, if the turn begins a task it has not carried, not
/// worn out by the daemon's thresholds. `Ok(true)` if it must compact first: a task it carried
/// was accepted, and compaction waits for the next turn, so a driver never needed again is not
/// paid to compact. One whose harness cannot compact is retired instead.
fn keep(ask: &Ask, agent: AgentId, warm: &Warm) -> Result<bool, EndWhy> {
    let Some(rec) = ask.world.project.driver(warm.driver) else {
        return Err(EndWhy::Unfit(PlannedReason::Disallowed));
    };
    let d = &rec.driver;
    let price = ask
        .world
        .daemon
        .harness(d.harness)
        .and_then(|h| h.models.iter().find(|m| m.model == d.model))
        .and_then(|m| m.levels.iter().find(|(e, _)| *e == d.effort))
        .map(|(_, p)| p);
    let c = Candidate {
        harness: d.harness,
        model: &d.model,
        effort: d.effort,
        price,
        machine: d.machine,
        login: None,
        holds_slot: true,
    };
    if rules::effort(ask, &c).is_err() {
        return Err(EndWhy::Unfit(if ask.range.contains(d.effort) {
            PlannedReason::EffortChanged
        } else {
            PlannedReason::RangeNarrowed
        }));
    }
    unfit(ask, &c)?;
    let task = ask.world.project.current_task(agent).map(|t| t.task.id);
    let new_task = task.is_none_or(|t| !warm.carried.contains(&t));
    if let Some(reason) = worn(&ask.world, rec, warm).filter(|_| new_task) {
        return Err(EndWhy::Retire(reason));
    }
    if !warm.accepted {
        return Ok(false);
    }
    let compacts = ask
        .world
        .daemon
        .harness(d.harness)
        .is_some_and(|h| h.support.compaction);
    if compacts {
        Ok(true)
    } else {
        Err(EndWhy::Retire(RetireReason::CannotCompact))
    }
}

/// Rules 2 and 3 for a driver that is already running: one the builder turned off, or one whose
/// harness and model are cooling off, takes no more turns. Core has no planned reason for a
/// cool-off, so it is recorded as disallowed, which is what the cool-off does to it meanwhile.
pub(crate) fn unfit(ask: &Ask, c: &Candidate) -> Result<(), EndWhy> {
    match rules::allowed(ask, c).and_then(|()| rules::cool_off(ask, c)) {
        Ok(()) => Ok(()),
        Err(_) => Err(EndWhy::Unfit(PlannedReason::Disallowed)),
    }
}

fn allocate(mut ask: Ask, ending: Option<Ending>) -> Result<Decision, Vec<Incompatible>> {
    let world = ask.world;
    // The warm driver's room is freed by ending it.
    if let Some(d) = ending.and_then(|e| world.project.driver(e.driver)) {
        ask.occupancy
            .free(&world.installed.slot(d.driver.harness, d.driver.machine));
    }
    let (candidates, no_level) = rules::candidates(&ask);
    let mut why: [Vec<Incompatible>; RULES.len()] = Default::default();
    if no_level {
        why[0].push(Incompatible::NoEffortInRange);
    }
    for c in &candidates {
        let Some((i, reason)) = RULES
            .iter()
            .enumerate()
            .find_map(|(i, rule)| rule(&ask, c).err().map(|r| (i, r)))
        else {
            return Ok(Decision::Start {
                choice: Choice {
                    harness: c.harness,
                    model: c.model.clone(),
                    effort: c.effort,
                    machine: c.machine,
                    view: world.installed.view(c.harness),
                },
                ending,
            });
        };
        if !why[i].contains(&reason) {
            why[i].push(reason);
        }
    }
    let reasons: Vec<_> = why.into_iter().flatten().collect();
    if reasons.is_empty() {
        // No harness on this machine offers anything at all.
        return Err(vec![Incompatible::NothingAllowed]);
    }
    Err(reasons)
}
