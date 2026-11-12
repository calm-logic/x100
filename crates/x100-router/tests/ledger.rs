//! Who pays for a turn, on what basis, and which budgets a charge takes to their limit.

mod common;

use common::*;
use x100_core::agent::{Agent, Position};
use x100_core::driver::{Driver, Purpose, StartReason, View};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, DriverId, TrialId};
use x100_core::ledger::{BudgetReached, ChargeSource, CostBasis, Payer};
use x100_core::money::{Money, Usage};
use x100_core::party::Actor;
use x100_core::trial::Arm;
use x100_router::bill;

fn worker(p: &mut Project, id: u128, parent: AgentId) -> AgentId {
    let id = AgentId::from_u128(id);
    p.b.push(
        Actor::Agent(parent),
        ProjectEvent::AgentAdded(Agent {
            id,
            role: format!("W{id}"),
            charter: "c".into(),
            position: Position::Worker {
                parent,
                effort: None,
            },
        }),
    );
    id
}

fn driver(agent: AgentId, purpose: Purpose) -> Driver {
    Driver {
        id: DriverId::from_u128(77),
        agent,
        harness: CHEAP,
        model: small().model,
        effort: Effort::Low,
        machine: MACHINE,
        purpose,
        view: View::Headless,
        why: StartReason::Wake,
    }
}

fn project() -> Project {
    Project::new(
        &[small()],
        EffortRange::FULL,
        Money::from_dollars(10),
        Money::from_dollars(50),
    )
}

#[test]
fn each_turn_is_charged_to_the_line_that_pays_for_its_agent() {
    let mut p = project();
    let (ceo, lead) = (p.ceo, p.lead);
    let under_lead = worker(&mut p, 10, lead);
    let below_that = worker(&mut p, 11, under_lead);
    let under_ceo = worker(&mut p, 12, ceo);
    let lab = Purpose::Lab {
        trial: TrialId::from_u128(1),
        arm: Arm::Changed,
        run: 1,
    };
    let cases = [
        (ceo, Purpose::Work, Payer::Project),
        (lead, Purpose::Work, Payer::Lead(lead)),
        (under_lead, Purpose::Work, Payer::Lead(lead)),
        (below_that, Purpose::Work, Payer::Lead(lead)),
        (under_ceo, Purpose::Work, Payer::Project),
        (lead, lab, Payer::Lab),
    ];
    for (agent, purpose, payer) in cases {
        let d = driver(agent, purpose);
        let usage = Usage {
            input: 10,
            output: 2,
            ..Usage::default()
        };
        let billed = bill(
            p.state(),
            &d,
            usage,
            Money::from_micros(5),
            CostBasis::Listed,
        )
        .expect("on the team");
        assert_eq!(billed.charge.payer, payer, "{agent}");
        assert_eq!(
            billed.charge.source,
            ChargeSource::Turn {
                driver: d.id,
                agent
            }
        );
        assert_eq!(
            (billed.charge.cost, billed.charge.basis, billed.charge.usage),
            (Money::from_micros(5), CostBasis::Listed, usage)
        );
    }
}

#[test]
fn a_charge_that_reaches_a_budget_says_so_once() {
    let mut p = project();
    let lead = p.lead;
    let d = driver(lead, Purpose::Work);
    let charge = |p: &Project, micros| {
        bill(
            p.state(),
            &d,
            Usage::default(),
            Money::from_micros(micros),
            CostBasis::Reported,
        )
        .unwrap()
    };
    assert!(charge(&p, 9_000_000).reached.is_empty());
    p.spend(Payer::Lead(lead), 9_000_000);
    assert_eq!(
        charge(&p, 1_000_000).reached,
        vec![BudgetReached::Lead(lead)]
    );
    p.spend(Payer::Lead(lead), 1_000_000);
    assert!(
        charge(&p, 1).reached.is_empty(),
        "a budget already reached is not reached again"
    );
    // The CEO's turn that takes the project to its total.
    let ceo = driver(p.ceo, Purpose::Work);
    let to_total = bill(
        p.state(),
        &ceo,
        Usage::default(),
        Money::from_dollars(40),
        CostBasis::Reported,
    )
    .unwrap();
    assert_eq!(to_total.reached, vec![BudgetReached::Project]);
}
