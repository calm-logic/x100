//! What becomes of a warm driver between turns: kept, or ended by the daemon's thresholds or by the
//! builder turning its model off.

mod common;

use std::time::Duration;

use common::*;
use x100_core::driver::{PlannedReason, RetireReason};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::DaemonEvent;
use x100_core::harness::Machine;
use x100_core::id::JobId;
use x100_core::inbox::InboxItem;
use x100_core::money::Money;
use x100_core::settings::DaemonSetting;
use x100_core::testing::START;
use x100_core::time::Timestamp;
use x100_router::{EndWhy, Upkeep, upkeep, warm_until};

struct Case {
    /// The lead still holds the task its driver worked on.
    mid_task: bool,
    context_used: u64,
    carried: usize,
    age: Duration,
    accepted: bool,
    compaction: bool,
    allowed: bool,
}

fn base() -> Case {
    Case {
        mid_task: false,
        context_used: 10_000,
        carried: 1,
        age: Duration::from_secs(60),
        accepted: false,
        compaction: true,
        allowed: true,
    }
}

fn run(c: &Case) -> Upkeep {
    let allowed = if c.allowed {
        vec![small(), mid()]
    } else {
        vec![mid()]
    };
    let mut p = Project::new(
        &allowed,
        EffortRange::FULL,
        Money::from_dollars(10),
        Money::from_dollars(50),
    );
    if c.mid_task {
        p.task(Effort::Low);
    } else {
        // Woken by a job, with no task open: between tasks.
        p.deliver(
            p.lead,
            InboxItem::Job {
                job: JobId::from_u128(9),
                note: "n".into(),
            },
        );
    }
    let mut warm = p.warm(small(), Effort::Low, c.context_used);
    warm.carried = (0..c.carried as u128)
        .map(x100_core::id::TaskId::from_u128)
        .collect();
    warm.accepted = c.accepted;
    let mut events = vec![DaemonEvent::MachineAdded(Machine {
        id: MACHINE,
        name: "local".into(),
        harnesses: vec![CHEAP, DEAR],
        cap: 8,
    })];
    events.extend(
        harnesses(c.compaction)
            .into_iter()
            .map(DaemonEvent::HarnessAdded),
    );
    events.push(DaemonEvent::SettingChanged(
        DaemonSetting::RetireAfterTasks(3),
    ));
    let daemon = fold_daemon(events);
    let installed = installed();
    let occupancy = occupied(1, None);
    let now = p.state().driver(warm.driver).unwrap().started + c.age;
    upkeep(
        &world(&daemon, p.state(), &installed, &occupancy, now),
        &warm,
    )
}

#[test]
fn warm_drivers_are_kept_or_ended_by_the_table() {
    let retire = |r| Upkeep::End(EndWhy::Retire(r));
    let four_hours = Duration::from_secs(4 * 3600);
    let cases: Vec<(&str, Case, Upkeep)> = vec![
        ("an ordinary warm driver is kept", base(), Upkeep::Keep),
        (
            "past the context threshold between tasks it is retired",
            Case {
                context_used: 150_000,
                ..base()
            },
            retire(RetireReason::ContextFull),
        ),
        (
            "at the context threshold exactly it is kept",
            Case {
                context_used: 140_000,
                ..base()
            },
            Upkeep::Keep,
        ),
        (
            "past the threshold mid-task it is kept: that is a planned handoff, not retirement",
            Case {
                context_used: 150_000,
                mid_task: true,
                ..base()
            },
            Upkeep::Keep,
        ),
        (
            "after carrying the set number of tasks it is retired",
            Case {
                carried: 3,
                ..base()
            },
            retire(RetireReason::TooManyTasks),
        ),
        (
            "once older than the set age it is retired",
            Case {
                age: four_hours,
                ..base()
            },
            retire(RetireReason::TooOld),
        ),
        (
            "after an accepted task it waits for its next turn to compact",
            Case {
                accepted: true,
                compaction: false,
                ..base()
            },
            Upkeep::Keep,
        ),
        (
            "a model the builder turned off finishes its turn and goes",
            Case {
                allowed: false,
                mid_task: true,
                ..base()
            },
            Upkeep::End(EndWhy::Unfit(PlannedReason::Disallowed)),
        ),
    ];
    let mut failed = Vec::new();
    for (name, case, expect) in cases {
        let got = run(&case);
        if got != expect {
            failed.push(format!("{name}: expected {expect:?}, got {got:?}"));
        }
    }
    assert!(failed.is_empty(), "\n{}", failed.join("\n"));
}

#[test]
fn the_warm_period_is_the_models_cache_lifetime() {
    let mut p = Project::new(
        &[small()],
        EffortRange::FULL,
        Money::from_dollars(10),
        Money::from_dollars(50),
    );
    p.task(Effort::Low);
    let warm = p.warm(small(), Effort::Low, 10);
    let daemon = daemon(8, &[]);
    let now = Timestamp::from_millis(START.as_millis() + 1_000);
    let rec = p.state().driver(warm.driver).unwrap();
    assert_eq!(
        warm_until(&daemon, &rec.driver, now),
        now + Duration::from_secs(300)
    );
}
