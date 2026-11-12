//! The invariants STATUS.md gives M8, over random sequences of charges, budget, range, allow-list
//! and cool-off changes, and turn starts: no turn starts once the budget that pays for it is
//! reached, and every driver that starts is compatible by all five rules at that moment. Also: the
//! router never says nothing fits while something does.
//!
//! The rules are checked here as the documents state them, apart from the router's own table.

mod common;

use common::*;
use proptest::prelude::*;
use x100_core::agent::{Agent, AgentEvent, Position};
use x100_core::driver::{DriverEvent, StartReason};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::{DaemonEvent, ProjectEvent};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::harness::{CoolOff, HarnessModel, Machine, ProviderFailure};
use x100_core::id::{AgentId, JobId};
use x100_core::ledger::{Budgets, Payer, Spend};
use x100_core::money::Money;
use x100_core::party::{Actor, Party};
use x100_core::project::SettingChange;
use x100_core::testing::START;
use x100_router::{Decision, EndWhy, Need, Occupancy, Warm, choose, needs};

const CAP: u32 = 3;

#[derive(Debug, Clone)]
enum Op {
    Charge { who: usize, micros: u64 },
    SetBudget { lead: usize, dollars: u64 },
    SetTotal { dollars: u64 },
    SetRange { who: usize, a: Effort, b: Effort },
    Allow { which: usize, on: bool },
    CoolOff { which: usize, on: bool },
    Turn { who: usize, effort: Effort },
}

fn effort() -> impl Strategy<Value = Effort> {
    prop::sample::select(Effort::ALL.to_vec())
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..4usize, 0..4_000_000u64).prop_map(|(who, micros)| Op::Charge { who, micros }),
        (0..2usize, 0..25u64).prop_map(|(lead, dollars)| Op::SetBudget { lead, dollars }),
        (0..60u64).prop_map(|dollars| Op::SetTotal { dollars }),
        (0..4usize, effort(), effort()).prop_map(|(who, a, b)| Op::SetRange { who, a, b }),
        (0..3usize, any::<bool>()).prop_map(|(which, on)| Op::Allow { which, on }),
        (0..3usize, any::<bool>()).prop_map(|(which, on)| Op::CoolOff { which, on }),
        (0..4usize, effort()).prop_map(|(who, effort)| Op::Turn { who, effort }),
        (0..4usize, effort()).prop_map(|(who, effort)| Op::Turn { who, effort }),
    ]
}

fn models() -> [HarnessModel; 3] {
    [small(), mid(), large()]
}

struct Run {
    p: Project,
    agents: [AgentId; 4],
    cooling: Vec<HarnessModel>,
}

impl Run {
    fn new() -> Run {
        let mut p = Project::new(
            &models(),
            EffortRange::FULL,
            Money::from_dollars(10),
            Money::from_dollars(30),
        );
        let (ceo, lead) = (p.ceo, p.lead);
        let other = AgentId::from_u128(3);
        let worker = AgentId::from_u128(4);
        let add = |p: &mut Project, id, role: &str, position| {
            p.b.push(
                Actor::Agent(ceo),
                ProjectEvent::AgentAdded(Agent {
                    id,
                    role: role.into(),
                    charter: "c".into(),
                    position,
                }),
            )
        };
        add(
            &mut p,
            other,
            "Product",
            Position::Lead {
                effort: EffortRange::FULL,
                budget: Money::from_dollars(10),
            },
        );
        add(
            &mut p,
            worker,
            "Backend",
            Position::Worker {
                parent: lead,
                effort: None,
            },
        );
        Run {
            p,
            agents: [ceo, lead, other, worker],
            cooling: Vec::new(),
        }
    }

    fn state(&self) -> &ProjectState {
        self.p.state()
    }

    fn daemon(&self) -> DaemonState {
        let mut events = vec![DaemonEvent::MachineAdded(Machine {
            id: MACHINE,
            name: "local".into(),
            harnesses: vec![CHEAP, DEAR],
            cap: CAP,
        })];
        events.extend(harnesses(true).into_iter().map(DaemonEvent::HarnessAdded));
        for (i, of) in self.cooling.iter().enumerate() {
            events.push(DaemonEvent::CoolOffStarted {
                cool_off: CoolOff {
                    of: of.clone(),
                    why: ProviderFailure::KeyRefused,
                    until: START,
                },
                ends: JobId::from_u128(500 + i as u128),
            });
        }
        fold_daemon(events)
    }

    fn apply(&mut self, op: &Op) -> Result<(), TestCaseError> {
        let leads = [self.agents[1], self.agents[2]];
        match *op {
            Op::Charge { who, micros } => {
                let payer = self.team().payer(self.agents[who]).unwrap();
                self.p.spend(payer, micros);
            }
            Op::SetBudget { lead, dollars } => {
                let team = self.team().clone();
                let op = x100_core::team::TeamOp::SetBudget {
                    agent: leads[lead],
                    budget: Money::from_dollars(dollars),
                };
                let settings = &self.state().project().unwrap().settings;
                if Budgets::in_force(settings, &team.apply(&op).unwrap())
                    .check_leads()
                    .is_ok()
                {
                    self.agent_event(
                        leads[lead],
                        AgentEvent::BudgetSet(Money::from_dollars(dollars)),
                    );
                }
            }
            Op::SetTotal { dollars } => {
                let mut settings = self.state().project().unwrap().settings.clone();
                settings.budget = Money::from_dollars(dollars);
                if Budgets::in_force(&settings, self.team())
                    .check_leads()
                    .is_ok()
                {
                    self.setting(SettingChange::Budget(Money::from_dollars(dollars)));
                }
            }
            Op::SetRange { who, a, b } => {
                self.agent_event(
                    self.agents[who],
                    AgentEvent::EffortSet(EffortRange::new(a.min(b), a.max(b)).unwrap()),
                );
            }
            Op::Allow { which, on } => self.setting(SettingChange::Allow {
                what: models()[which].clone(),
                allowed: on,
            }),
            Op::CoolOff { which, on } => {
                let of = models()[which].clone();
                self.cooling.retain(|c| *c != of);
                if on {
                    self.cooling.push(of);
                }
            }
            Op::Turn { who, effort } => self.turn(self.agents[who], effort)?,
        }
        Ok(())
    }

    fn team(&self) -> &x100_core::team::Team {
        &self.state().project().unwrap().team
    }

    fn agent_event(&mut self, agent: AgentId, event: AgentEvent) {
        self.p
            .b
            .push(Actor::Builder, ProjectEvent::Agent { agent, event });
    }

    fn setting(&mut self, change: SettingChange) {
        self.p
            .b
            .push(Actor::Builder, ProjectEvent::SettingChanged(change));
    }

    /// A new task reaches `agent`, and its next turn is decided and, if a driver is found, run.
    fn turn(&mut self, agent: AgentId, effort: Effort) -> Result<(), TestCaseError> {
        let from = match self.team().parent(agent) {
            Some(parent) => Party::Agent(parent),
            None => Party::Builder,
        };
        self.p.b.open_delegated(from, agent, "t", "a", effort);
        let daemon = self.daemon();
        let installed = installed();
        let occupancy = Occupancy::count([self.state()], &installed);
        let warm = self.state().acting_driver(agent).map(|driver| Warm {
            driver,
            carried: Default::default(),
            accepted: false,
        });
        let need = Need {
            agent,
            purpose: x100_core::driver::Purpose::Work,
            warm: warm.clone(),
        };
        let decision = choose(
            &world(&daemon, self.state(), &installed, &occupancy, START),
            &need,
        );
        let fits = |of: &HarnessModel, effort: Effort, holds_slot: bool| {
            compatible(&daemon, self.state(), agent, of, effort, holds_slot)
        };
        match decision {
            Ok(Decision::Reuse { driver: d, .. }) => {
                prop_assert_eq!(Some(d), warm.map(|w| w.driver));
                let rec = &self.state().driver(d).unwrap().driver;
                let of = HarnessModel {
                    harness: rec.harness,
                    model: rec.model.clone(),
                };
                prop_assert!(fits(&of, rec.effort, true), "reused an incompatible driver");
                self.run_turn(d);
            }
            Ok(Decision::Start { choice, ending }) => {
                let of = HarnessModel {
                    harness: choice.harness,
                    model: choice.model.clone(),
                };
                prop_assert_eq!(ending.map(|e| e.driver), warm.map(|w| w.driver));
                prop_assert!(
                    fits(&of, choice.effort, false),
                    "started an incompatible driver"
                );
                if let Some(e) = ending {
                    self.end(e.driver, e.why);
                }
                let d = self
                    .p
                    .b
                    .start_driver(agent, of, choice.effort, StartReason::Wake);
                self.run_turn(d);
            }
            Err(why) => {
                let any = models().iter().any(|of| {
                    Effort::ALL
                        .iter()
                        .any(|e| offered(of, *e) && fits(of, *e, false))
                });
                let warm_fits = warm.is_some_and(|w| {
                    let rec = &self.state().driver(w.driver).unwrap().driver;
                    fits(
                        &HarnessModel {
                            harness: rec.harness,
                            model: rec.model.clone(),
                        },
                        rec.effort,
                        true,
                    )
                });
                prop_assert!(
                    !any && !warm_fits,
                    "said nothing fits ({why:?}) while something did"
                );
            }
        }
        Ok(())
    }

    fn end(&mut self, driver: x100_core::id::DriverId, why: EndWhy) {
        let ev = |event| ProjectEvent::Driver { driver, event };
        match why {
            EndWhy::Retire(r) => self.p.b.push(Actor::Daemon, ev(DriverEvent::Retired(r))),
            EndWhy::Unfit(r) => {
                self.p
                    .b
                    .push(Actor::Daemon, ev(DriverEvent::HandoffStarted(r)));
                self.p.b.push(Actor::Daemon, ev(DriverEvent::TurnEnded));
            }
        }
    }

    fn run_turn(&mut self, driver: x100_core::id::DriverId) {
        let agent = self.state().driver(driver).unwrap().driver.agent;
        let given = self.state().agent(agent).unwrap().inbox.waiting().collect();
        let ev = |event| ProjectEvent::Driver { driver, event };
        self.p
            .b
            .push(Actor::Daemon, ev(DriverEvent::TurnStarted { given }));
        self.p.b.push(Actor::Daemon, ev(DriverEvent::TurnEnded));
    }
}

fn offered(of: &HarnessModel, effort: Effort) -> bool {
    harnesses(true)
        .iter()
        .filter(|h| h.id == of.harness)
        .flat_map(|h| &h.models)
        .any(|m| m.model == of.model && m.levels.iter().any(|(e, _)| *e == effort))
}

/// "Finding a driver", rule by rule, as the architecture states it. The agent's own live driver
/// is not counted against the machine, since ending it frees its room.
fn compatible(
    daemon: &DaemonState,
    state: &ProjectState,
    agent: AgentId,
    of: &HarnessModel,
    effort: Effort,
    holds_slot: bool,
) -> bool {
    let project = state.project().unwrap();
    let range = project.team.effort(agent).unwrap();
    let rule1 = range.admits(effort, needs(state, agent, range));
    let rule2 = project.settings.allowed.contains(of);
    let rule3 = daemon.cooling_off(of).is_none();
    let others = state
        .live_drivers()
        .filter(|d| d.driver.agent != agent)
        .count() as u32;
    let rule4 = holds_slot || others < CAP;
    let view = state.budget().unwrap();
    let spend = Spend {
        project: view.project.spent,
        lab: view.lab.spent,
        leads: view.leads.iter().map(|(id, l)| (*id, l.spent)).collect(),
    };
    let payer: Payer = project.team.payer(agent).unwrap();
    let rule5 = Budgets::in_force(&project.settings, &project.team)
        .admits(&spend, payer)
        .is_ok();
    rule1 && rule2 && rule3 && rule4 && rule5
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn no_turn_starts_past_its_budget_and_every_driver_is_compatible(ops in prop::collection::vec(op(), 1..60)) {
        let mut run = Run::new();
        for op in &ops {
            run.apply(op)?;
        }
    }
}
