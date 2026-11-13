//! The choice as a table: ranges, allow-lists, cool-offs, full machines and logins, budgets, the
//! warm driver, and nothing compatible with every reason why.

mod common;

use common::*;
use x100_core::driver::{Incompatible, Purpose, RetireReason, Unfit};
use x100_core::effort::{Effort, EffortRange};
use x100_core::harness::HarnessModel;
use x100_core::harness::Login;
use x100_core::id::TrialId;
use x100_core::ledger::{BudgetReached, Payer};
use x100_core::money::Money;
use x100_core::testing::START;
use x100_core::trial::Arm;
use x100_router::{Decision, Need, choose, slot};

use Effort::*;

#[derive(Clone)]
struct Setup {
    allowed: Vec<HarnessModel>,
    range: (Effort, Effort),
    needs: Effort,
    budget: u64,
    total: u64,
    lead_spent: u64,
    project_spent: u64,
    lab_spent: u64,
    cooling: Vec<HarnessModel>,
    /// Models whose drivers already failed on the task.
    avoid: Vec<HarnessModel>,
    cap: u32,
    occupied: u32,
    /// The cheap harness runs in a terminal on a login with this many drivers live.
    /// The cheap harness on a login: its cap per login, and how many drivers it has live.
    cheap_login: Option<(u32, u32)>,
    /// The lead's warm driver: its model, effort and context used.
    warm: Option<(HarnessModel, Effort, u64)>,
    lab: bool,
    /// The builder narrows the range after the warm driver started.
    narrow_to: Option<(Effort, Effort)>,
    /// A task the warm driver carried was accepted since it last compacted.
    accepted: bool,
    compaction: bool,
}

fn base() -> Setup {
    Setup {
        allowed: vec![small(), mid(), large()],
        range: (Low, Max),
        needs: Low,
        budget: 10,
        total: 50,
        lead_spent: 0,
        project_spent: 0,
        lab_spent: 0,
        cooling: vec![],
        avoid: vec![],
        cap: 8,
        occupied: 0,
        cheap_login: None,
        warm: None,
        lab: false,
        narrow_to: None,
        accepted: false,
        compaction: true,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Expect {
    Start(HarnessModel, Effort),
    Replace(HarnessModel, Effort, RetireReason),
    Reuse,
    CompactThenReuse,
    Waits(Vec<Incompatible>),
}

const CHEAP_LOGIN: &str = "/home/builder/.cheap";

const LEAD: x100_core::id::AgentId = x100_core::id::AgentId::from_u128(2);

fn run(s: &Setup) -> Expect {
    let range = EffortRange::new(s.range.0, s.range.1).unwrap();
    let mut p = Project::new(
        &s.allowed,
        range,
        Money::from_dollars(s.budget),
        Money::from_dollars(s.total),
    );
    let warm = s.warm.clone().map(|(of, effort, used)| {
        // The warm driver carried an earlier task; the one deciding now is new.
        p.task(Low);
        let mut warm = p.warm(of, effort, used);
        warm.accepted = s.accepted;
        warm
    });
    p.task(s.needs);
    if let Some((min, max)) = s.narrow_to {
        p.set_effort(p.lead, EffortRange::new(min, max).unwrap());
    }
    if s.lead_spent > 0 {
        p.spend(Payer::Lead(p.lead), s.lead_spent);
    }
    if s.project_spent > 0 {
        p.spend(Payer::Project, s.project_spent);
    }
    if s.lab_spent > 0 {
        p.spend(Payer::Lab, s.lab_spent);
    }
    let login = s.cheap_login.map(|(cap, _)| (CHEAP_LOGIN, cap));
    let daemon = daemon_on_login(s.cap, &s.cooling, s.compaction, login);
    let installed = installed();
    let mut occupancy = occupied(s.occupied, None);
    for _ in 0..s.cheap_login.map_or(0, |(_, live)| live) {
        occupancy.take(&x100_router::Slot {
            machine: MACHINE,
            login: Some(Login(CHEAP_LOGIN.into())),
        });
    }
    if let Some(w) = &warm {
        let harness = p.state().driver(w.driver).unwrap().driver.harness;
        occupancy.take(&slot(&daemon, harness, MACHINE));
    }
    let need = Need {
        agent: p.lead,
        purpose: if s.lab {
            Purpose::Lab {
                trial: TrialId::from_u128(1),
                arm: Arm::Changed,
                run: 1,
            }
        } else {
            Purpose::Work
        },
        warm,
        avoid: s.avoid.iter().cloned().collect(),
    };
    let w = world(&daemon, p.state(), &installed, &occupancy, START);
    let model = |c: &x100_router::Choice| HarnessModel {
        harness: c.harness,
        model: c.model.clone(),
    };
    match choose(&w, &need) {
        Ok(Decision::Reuse { compact: false, .. }) => Expect::Reuse,
        Ok(Decision::Reuse { compact: true, .. }) => Expect::CompactThenReuse,
        Ok(Decision::Start {
            choice,
            ending: None,
        }) => Expect::Start(model(&choice), choice.effort),
        Ok(Decision::Start {
            choice,
            ending: Some(e),
        }) => Expect::Replace(model(&choice), choice.effort, e.why),
        Err(why) => Expect::Waits(why),
    }
}

#[test]
fn the_choice_follows_the_table() {
    let budget = |r| Incompatible::Budget(r);
    let cases: Vec<(&str, Setup, Expect)> = vec![
        (
            "a low task runs on the cheapest model",
            base(),
            Expect::Start(small(), Low),
        ),
        (
            "a task needing high runs on the cheapest model that offers it",
            Setup {
                needs: High,
                ..base()
            },
            Expect::Start(mid(), High),
        ),
        (
            "a task needing max goes to the dear provider",
            Setup {
                needs: Max,
                ..base()
            },
            Expect::Start(large(), Max),
        ),
        (
            "a task above the range runs at the top of it",
            Setup {
                range: (Low, Medium),
                needs: Max,
                ..base()
            },
            Expect::Start(small(), Medium),
        ),
        (
            "a task below the range runs at its floor",
            Setup {
                range: (High, Max),
                needs: Low,
                ..base()
            },
            Expect::Start(mid(), High),
        ),
        (
            "a disallowed provider is skipped",
            Setup {
                allowed: vec![large()],
                ..base()
            },
            Expect::Start(large(), Low),
        ),
        (
            "a model cooling off is skipped for the next cheapest, at an effort above the need",
            Setup {
                cooling: vec![small()],
                ..base()
            },
            Expect::Start(mid(), Medium),
        ),
        (
            "a model whose driver failed on the task is skipped for the next cheapest",
            Setup {
                avoid: vec![small()],
                ..base()
            },
            Expect::Start(mid(), Medium),
        ),
        (
            "a skip that leaves nothing compatible is dropped",
            Setup {
                allowed: vec![small(), mid()],
                avoid: vec![small(), mid()],
                ..base()
            },
            Expect::Start(small(), Low),
        ),
        (
            "a skip never makes a driver that fails a rule compatible",
            Setup {
                allowed: vec![small()],
                cooling: vec![small()],
                avoid: vec![small()],
                ..base()
            },
            Expect::Waits(vec![
                Incompatible::NothingAllowed,
                Incompatible::CoolingOff(small()),
            ]),
        ),
        (
            "a login at its cap of terminal drivers is skipped",
            Setup {
                cheap_login: Some((4, 4)),
                ..base()
            },
            Expect::Start(large(), Low),
        ),
        (
            "a login under its cap is used",
            Setup {
                cheap_login: Some((4, 3)),
                ..base()
            },
            Expect::Start(small(), Low),
        ),
        (
            "the cap per login is the daemon's setting",
            Setup {
                cheap_login: Some((1, 1)),
                ..base()
            },
            Expect::Start(large(), Low),
        ),
        (
            "with the dear provider not allowed and the login full, the agent waits and is told both",
            Setup {
                cheap_login: Some((2, 2)),
                allowed: vec![small(), mid()],
                ..base()
            },
            Expect::Waits(vec![
                Incompatible::NothingAllowed,
                Incompatible::LoginFull {
                    login: Login(CHEAP_LOGIN.into()),
                },
            ]),
        ),
        (
            "a full machine starts nothing",
            Setup {
                cap: 3,
                occupied: 3,
                ..base()
            },
            Expect::Waits(vec![Incompatible::MachineFull { machine: MACHINE }]),
        ),
        (
            "nothing allowed",
            Setup {
                allowed: vec![],
                ..base()
            },
            Expect::Waits(vec![Incompatible::NothingAllowed]),
        ),
        (
            "nothing in range among what is allowed: every rule that excluded something says so",
            Setup {
                range: (Max, Max),
                allowed: vec![small(), mid()],
                ..base()
            },
            Expect::Waits(vec![
                Incompatible::NoEffortInRange,
                Incompatible::NothingAllowed,
            ]),
        ),
        (
            "everything allowed is cooling off",
            Setup {
                allowed: vec![small(), mid()],
                cooling: vec![small(), mid()],
                ..base()
            },
            Expect::Waits(vec![
                Incompatible::NothingAllowed,
                Incompatible::CoolingOff(small()),
                Incompatible::CoolingOff(mid()),
            ]),
        ),
        (
            "a lead at its budget starts nothing",
            Setup {
                lead_spent: 10_000_000,
                ..base()
            },
            Expect::Waits(vec![budget(BudgetReached::Lead(LEAD))]),
        ),
        (
            "a lead just under its budget starts",
            Setup {
                lead_spent: 9_999_999,
                ..base()
            },
            Expect::Start(small(), Low),
        ),
        (
            "a project at its total starts nothing, whatever the lead has left",
            Setup {
                project_spent: 50_000_000,
                ..base()
            },
            Expect::Waits(vec![budget(BudgetReached::Project)]),
        ),
        (
            "a lab driver stops at the lab's cap",
            // The lab's cap is a tenth of the total: five dollars of fifty.
            Setup {
                lab: true,
                lab_spent: 5_000_000,
                ..base()
            },
            Expect::Waits(vec![budget(BudgetReached::Lab)]),
        ),
        (
            "a compatible warm driver is reused",
            Setup {
                warm: Some((small(), Low, 10_000)),
                ..base()
            },
            Expect::Reuse,
        ),
        (
            "a warm driver is reused for more effort while its range allows its level",
            Setup {
                warm: Some((large(), Max, 10_000)),
                needs: High,
                ..base()
            },
            Expect::Reuse,
        ),
        (
            "a task needing more effort than the warm driver has gets a new driver",
            Setup {
                warm: Some((small(), Low, 10_000)),
                needs: High,
                ..base()
            },
            Expect::Replace(mid(), High, RetireReason::Unfit(Unfit::EffortChanged)),
        ),
        (
            "a warm driver below a narrowed range gets a new driver",
            Setup {
                warm: Some((small(), Low, 10_000)),
                narrow_to: Some((High, Max)),
                ..base()
            },
            Expect::Replace(mid(), High, RetireReason::Unfit(Unfit::RangeNarrowed)),
        ),
        (
            "a warm driver past the context threshold is retired before a new task",
            Setup {
                warm: Some((small(), Low, 150_000)),
                ..base()
            },
            Expect::Replace(small(), Low, RetireReason::ContextFull),
        ),
        (
            "a warm driver whose model was turned off gets a new driver",
            Setup {
                warm: Some((small(), Low, 10_000)),
                allowed: vec![mid(), large()],
                ..base()
            },
            Expect::Replace(mid(), Medium, RetireReason::Unfit(Unfit::Disallowed)),
        ),
        (
            "a warm driver whose model cools off gets a new driver",
            Setup {
                warm: Some((small(), Low, 10_000)),
                cooling: vec![small()],
                ..base()
            },
            Expect::Replace(mid(), Medium, RetireReason::Unfit(Unfit::CoolingOff)),
        ),
        (
            "a warm driver on a full machine is still reused: it holds its room",
            Setup {
                warm: Some((small(), Low, 10_000)),
                cap: 1,
                ..base()
            },
            Expect::Reuse,
        ),
        (
            "after a task it carried was accepted, a warm driver compacts before its next turn",
            Setup {
                warm: Some((small(), Low, 10_000)),
                accepted: true,
                ..base()
            },
            Expect::CompactThenReuse,
        ),
        (
            "after an accepted task, one whose harness cannot compact is retired for a new one",
            Setup {
                warm: Some((small(), Low, 10_000)),
                accepted: true,
                compaction: false,
                ..base()
            },
            Expect::Replace(small(), Low, RetireReason::CannotCompact),
        ),
        (
            "a warm driver takes no turn once the budget is reached",
            Setup {
                warm: Some((small(), Low, 10_000)),
                lead_spent: 10_000_000,
                ..base()
            },
            Expect::Waits(vec![budget(BudgetReached::Lead(LEAD))]),
        ),
        (
            "an unfit warm driver is not ended when nothing else would do",
            Setup {
                warm: Some((small(), Low, 10_000)),
                needs: High,
                allowed: vec![small()],
                ..base()
            },
            Expect::Waits(vec![
                Incompatible::NoEffortInRange,
                Incompatible::NothingAllowed,
            ]),
        ),
    ];
    let mut failed = Vec::new();
    for (name, setup, expect) in cases {
        let got = run(&setup);
        if got != expect {
            failed.push(format!(
                "{name}:\n  expected {expect:?}\n  got      {got:?}"
            ));
        }
    }
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}

#[test]
fn ties_go_to_the_lower_effort_then_the_harness_then_the_model() {
    // `small` at Low and at Medium cost the same: Low wins.
    assert_eq!(run(&base()), Expect::Start(small(), Low));
    // Two harnesses with the same price at the same effort: the lower harness id wins, every time.
    let first = run(&Setup {
        allowed: vec![mid(), large()],
        needs: Medium,
        ..base()
    });
    assert_eq!(first, Expect::Start(mid(), Medium));
}

#[test]
fn changing_a_range_or_a_budget_changes_the_next_decision() {
    let before = run(&Setup {
        range: (Low, Medium),
        needs: Max,
        ..base()
    });
    let after = run(&Setup {
        range: (Low, Max),
        needs: Max,
        ..base()
    });
    assert_eq!(before, Expect::Start(small(), Medium));
    assert_eq!(after, Expect::Start(large(), Max));
    let stopped = run(&Setup {
        lead_spent: 10_000_000,
        ..base()
    });
    let raised = run(&Setup {
        lead_spent: 10_000_000,
        budget: 20,
        ..base()
    });
    assert!(matches!(stopped, Expect::Waits(_)));
    assert_eq!(raised, Expect::Start(small(), Low));
}
