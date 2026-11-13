//! States for the router's tests, built with core's testing helpers: a daemon with two providers
//! at different prices on one machine, and a project with a CEO, one lead and a task for it.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::time::Duration;

use x100_core::agent::{Agent, AgentEvent, Position};
use x100_core::driver::{ContextFill, DriverEvent, StartReason};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent, Seq};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::harness::Login;
use x100_core::harness::{
    CoolOff, Harness, HarnessKind, HarnessModel, Machine, ModelName, ModelOffer, ProviderFailure,
    Support,
};
use x100_core::id::{AgentId, DriverId, HarnessId, JobId, MachineId, OneOffCallId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::ledger::{Charge, ChargeSource, CostBasis, Payer};
use x100_core::money::{Money, Percent, Price, Rates};
use x100_core::party::{Actor, Party};
use x100_core::project::Settings;
use x100_core::settings::DaemonSetting;
use x100_core::settings::DaemonSettings;
use x100_core::testing::{LogBuilder, START};
use x100_core::time::Timestamp;
use x100_router::{Installed, Occupancy, Slot, Warm, World};

pub const MACHINE: MachineId = MachineId::from_u128(1);
pub const CHEAP: HarnessId = HarnessId::from_u128(2);
pub const DEAR: HarnessId = HarnessId::from_u128(3);

pub fn small() -> HarnessModel {
    on(CHEAP, "small")
}
pub fn mid() -> HarnessModel {
    on(CHEAP, "mid")
}
pub fn large() -> HarnessModel {
    on(DEAR, "large")
}

pub fn on(harness: HarnessId, model: &str) -> HarnessModel {
    HarnessModel {
        harness,
        model: ModelName(model.into()),
    }
}

/// Dollars per million tokens.
fn price(input: u64, output: u64) -> Price {
    Price {
        rates: Rates {
            input: Money::from_dollars(input),
            output: Money::from_dollars(output),
            cache_read: Money::ZERO,
            cache_write_short: Money::ZERO,
            cache_write_long: Money::ZERO,
        },
        long_context: None,
    }
}

fn offer(model: &str, levels: &[Effort], input: u64, output: u64) -> ModelOffer {
    ModelOffer {
        model: ModelName(model.into()),
        context_window: 200_000,
        cache_lifetime: Duration::from_secs(300),
        levels: levels.iter().map(|e| (*e, price(input, output))).collect(),
    }
}

/// Two providers. The cheap one runs `small` at Low and Medium and `mid` at Medium and High; the
/// dear one runs `large` at every level, at five times `small`'s price.
pub fn harnesses(compaction: bool) -> Vec<Harness> {
    use Effort::*;
    let support = Support {
        further_turns: true,
        compaction,
        context_fill: true,
        permission_hook: false,
    };
    vec![
        Harness {
            id: CHEAP,
            kind: HarnessKind::Custom {
                name: "cheap".into(),
            },
            support,
            models: vec![
                offer("small", &[Low, Medium], 1, 5),
                offer("mid", &[Medium, High], 2, 10),
            ],
            login: None,
        },
        Harness {
            id: DEAR,
            kind: HarnessKind::Custom {
                name: "dear".into(),
            },
            support,
            models: vec![offer("large", &Effort::ALL, 5, 25)],
            login: None,
        },
    ]
}

pub fn daemon(cap: u32, cooling: &[HarnessModel]) -> DaemonState {
    daemon_with(cap, cooling, true)
}

pub fn daemon_with(cap: u32, cooling: &[HarnessModel], compaction: bool) -> DaemonState {
    daemon_on_login(cap, cooling, compaction, None)
}

/// The cheap harness on a login, with the daemon's cap on terminal drivers per login.
pub fn daemon_on_login(
    cap: u32,
    cooling: &[HarnessModel],
    compaction: bool,
    login: Option<(&str, u32)>,
) -> DaemonState {
    let mut events = vec![DaemonEvent::MachineAdded(Machine {
        id: MACHINE,
        name: "local".into(),
        harnesses: vec![CHEAP, DEAR],
        cap,
        passed_over: Vec::new(),
    })];
    events.extend(harnesses(compaction).into_iter().map(|mut h| {
        if h.id == CHEAP {
            h.login = login.map(|(l, _)| Login(l.into()));
        }
        DaemonEvent::HarnessAdded(h)
    }));
    if let Some((_, per_login)) = login {
        events.push(DaemonEvent::SettingChanged(
            DaemonSetting::TerminalDriversPerLogin(per_login),
        ));
    }
    for (i, of) in cooling.iter().enumerate() {
        events.push(DaemonEvent::CoolOffStarted {
            cool_off: CoolOff {
                of: of.clone(),
                why: ProviderFailure::RateLimited { retry_after: None },
                until: START + Duration::from_secs(600),
            },
            ends: JobId::from_u128(100 + i as u128),
        });
    }
    fold_daemon(events)
}

pub fn fold_daemon(events: Vec<DaemonEvent>) -> DaemonState {
    let log: Vec<_> = events
        .into_iter()
        .enumerate()
        .map(|(i, event)| Envelope {
            seq: Seq(i as u64 + 1),
            at: START,
            by: Actor::Daemon,
            event,
        })
        .collect();
    DaemonState::fold(&log).expect("the daemon's log folds")
}

pub fn installed() -> Installed {
    Installed {
        machine: MACHINE,
        harnesses: BTreeSet::from([CHEAP, DEAR]),
    }
}

pub fn occupied(n: u32, login: Option<&str>) -> Occupancy {
    let mut o = Occupancy::default();
    for _ in 0..n {
        o.take(&Slot {
            machine: MACHINE,
            login: login.map(|l| Login(l.into())),
        });
    }
    o
}

/// A project: the CEO, one lead with `range` and `budget`, and a project total of `total`.
pub struct Project {
    pub b: LogBuilder,
    pub ceo: AgentId,
    pub lead: AgentId,
}

impl Project {
    pub fn new(allowed: &[HarnessModel], range: EffortRange, budget: Money, total: Money) -> Self {
        let mut b = LogBuilder::new(START);
        let ceo = AgentId::from_u128(1);
        let lead = AgentId::from_u128(2);
        b.push(
            Actor::Builder,
            ProjectEvent::Created {
                ceo,
                charter: "c".into(),
                effort: EffortRange::FULL,
                settings: Settings {
                    ask_me_about: x100_core::action::AskMeAbout::HighRiskOnly,
                    budget: total,
                    lab_share: Percent::new(10).unwrap(),
                    allowed: allowed.iter().cloned().collect(),
                },
            },
        );
        b.push(
            Actor::Agent(ceo),
            ProjectEvent::AgentAdded(Agent {
                id: lead,
                role: "Tech".into(),
                charter: "c".into(),
                position: Position::Lead {
                    effort: range,
                    budget,
                },
                appearance: Default::default(),
            }),
        );
        Project { b, ceo, lead }
    }

    /// A task for the lead from the CEO, needing `effort`.
    pub fn task(&mut self, effort: Effort) -> TaskId {
        self.b
            .open_delegated(Party::Agent(self.ceo), self.lead, "t", "a", effort)
    }

    pub fn spend(&mut self, payer: Payer, micros: u64) {
        let call = self.b.id(OneOffCallId::new);
        self.b.push(
            Actor::Daemon,
            ProjectEvent::Charged(Charge {
                payer,
                source: ChargeSource::OneOff { call },
                usage: Default::default(),
                cost: Money::from_micros(micros),
                basis: CostBasis::Reported,
            }),
        );
    }

    /// The lead's driver on `of` at `effort`, after one turn on its current task, warm now.
    pub fn warm(&mut self, of: HarnessModel, effort: Effort, context_used: u64) -> Warm {
        let task = self.b.state().current_task(self.lead).map(|t| t.task.id);
        let driver = self
            .b
            .start_driver(self.lead, of, effort, StartReason::Wake);
        let given = self
            .b
            .state()
            .agent(self.lead)
            .unwrap()
            .inbox
            .waiting()
            .collect();
        let ev = |event| ProjectEvent::Driver { driver, event };
        self.b
            .push(Actor::Daemon, ev(DriverEvent::TurnStarted { given }));
        self.b.push(
            Actor::Daemon,
            ev(DriverEvent::ContextReported(ContextFill {
                used: context_used,
                window: 200_000,
            })),
        );
        self.b.push(Actor::Daemon, ev(DriverEvent::TurnEnded));
        Warm {
            driver,
            carried: task.into_iter().collect(),
            accepted: false,
        }
    }

    pub fn set_effort(&mut self, agent: AgentId, range: EffortRange) {
        self.b.push(
            Actor::Builder,
            ProjectEvent::Agent {
                agent,
                event: AgentEvent::EffortSet(range),
            },
        );
    }

    pub fn deliver(&mut self, agent: AgentId, what: InboxItem) {
        self.b.deliver(agent, what);
    }

    pub fn state(&self) -> &ProjectState {
        self.b.state()
    }
}

pub fn world<'a>(
    daemon: &'a DaemonState,
    project: &'a ProjectState,
    installed: &'a Installed,
    occupancy: &'a Occupancy,
    now: Timestamp,
) -> World<'a> {
    World {
        daemon,
        project,
        installed,
        occupancy,
        now,
    }
}

pub fn defaults() -> DaemonSettings {
    DaemonSettings::default()
}

pub fn driver_id(n: u128) -> DriverId {
    DriverId::from_u128(n)
}
