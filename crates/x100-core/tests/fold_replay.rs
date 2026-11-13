//! Replaying a log: a snapshot plus the later events gives the state the whole log gives, folding is
//! deterministic, and an event the fold cannot apply is an error that names it.

use proptest::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use x100_core::agent::{AgentEvent, Position};
use x100_core::driver::{Driver, DriverEvent, Purpose, StartReason, View};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent, Seq};
use x100_core::fold::{Corrupt, DaemonState, FoldError, ProjectState, Record};
use x100_core::git::CommitId;
use x100_core::id::{AgentId, DriverId, TrialId};
use x100_core::ledger::LeadsOverTotal;
use x100_core::machine::{MachineKind, Rejected};
use x100_core::memory::SkillVersion;
use x100_core::money::Money;
use x100_core::party::Actor;
use x100_core::task::{Outcome, Report, TaskEvent};
use x100_core::testing::{Fernleaf, MACHINE, daemon_log, fernleaf, opus};
use x100_core::time::Timestamp;

trait Fold: Default + Clone + PartialEq + std::fmt::Debug + Serialize + DeserializeOwned {
    type Event: Clone;
    fn apply(&mut self, e: &Envelope<Self::Event>) -> Result<(), FoldError>;
}

impl Fold for ProjectState {
    type Event = ProjectEvent;
    fn apply(&mut self, e: &Envelope<ProjectEvent>) -> Result<(), FoldError> {
        ProjectState::apply(self, e).map(|_| ())
    }
}

impl Fold for DaemonState {
    type Event = DaemonEvent;
    fn apply(&mut self, e: &Envelope<DaemonEvent>) -> Result<(), FoldError> {
        DaemonState::apply(self, e).map(|_| ())
    }
}

fn fold<S: Fold>(state: &mut S, events: &[Envelope<S::Event>]) {
    for e in events {
        state.apply(e).unwrap();
    }
}

fn through_json<S: Fold>(state: &S) -> S {
    serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap()
}

/// Folds the log, sending a snapshot through JSON at each of `splits`, and returns the state.
fn with_snapshots<S: Fold>(log: &[Envelope<S::Event>], splits: &[usize]) -> S {
    let mut state = S::default();
    let mut at = 0;
    for &split in splits {
        fold(&mut state, &log[at..split]);
        state = through_json(&state);
        at = split;
    }
    fold(&mut state, &log[at..]);
    state
}

fn every_split_gives_the_whole_logs_state<S: Fold>(log: &[Envelope<S::Event>]) {
    let mut whole = S::default();
    fold(&mut whole, log);
    let mut prefix = S::default();
    for split in 0..=log.len() {
        let mut resumed = through_json(&prefix);
        fold(&mut resumed, &log[split..]);
        assert_eq!(resumed, whole, "a snapshot after {split} events");
        if split < log.len() {
            prefix.apply(&log[split]).unwrap();
        }
    }
}

#[test]
fn a_project_snapshot_at_any_point_plus_the_rest_gives_the_whole_logs_state() {
    every_split_gives_the_whole_logs_state::<ProjectState>(&fernleaf().log);
}

#[test]
fn a_daemon_snapshot_at_any_point_plus_the_rest_gives_the_whole_logs_state() {
    every_split_gives_the_whole_logs_state::<DaemonState>(&daemon_log(Timestamp::from_millis(0)));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Several snapshots in one replay, as a client that reconnects more than once sees.
    #[test]
    fn any_number_of_snapshots_gives_the_same_state(
        splits in prop::collection::btree_set(0usize..=680, 0..6)
    ) {
        let log = fernleaf().log;
        let splits: Vec<_> = splits.into_iter().filter(|s| *s <= log.len()).collect();
        let whole = ProjectState::fold(&log).unwrap();
        prop_assert_eq!(with_snapshots::<ProjectState>(&log, &splits), whole);
    }

    /// Folding the same prefix twice gives the same state and the same bytes.
    #[test]
    fn folding_is_deterministic(len in 0usize..=680) {
        let log = fernleaf().log;
        let prefix = &log[..len.min(log.len())];
        let a = ProjectState::fold(prefix).unwrap();
        let b = ProjectState::fold(prefix).unwrap();
        prop_assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
        prop_assert_eq!(a, b);
    }
}

#[test]
fn the_scenario_is_the_same_log_every_time() {
    assert_eq!(fernleaf().log, fernleaf().log);
}

/// The scenario's log with `event` appended as the next one.
fn appended(
    f: &Fernleaf,
    by: Actor,
    event: ProjectEvent,
) -> (ProjectState, Envelope<ProjectEvent>) {
    let state = ProjectState::fold(&f.log).unwrap();
    let last = f.log.last().unwrap();
    let e = Envelope {
        seq: Seq(last.seq.0 + 1),
        at: last.at,
        by,
        event,
    };
    (state, e)
}

#[test]
fn an_event_a_machine_rejects_is_an_error_naming_the_record() {
    let f = fernleaf();
    let report = TaskEvent::Reported(Report {
        outcome: Outcome::Completed,
        summary: "again".into(),
    });
    let event = ProjectEvent::Task {
        task: f.smoke,
        event: report,
    };
    let (mut state, e) = appended(&f, Actor::Agent(f.tech), event);
    let err = state.apply(&e).unwrap_err();
    assert_eq!(
        err,
        FoldError {
            seq: e.seq,
            record: Some(Record::Task(f.smoke)),
            cause: Corrupt::Rejected(Rejected {
                machine: MachineKind::Task,
                state: "Done",
                event: "Reported",
            }),
        }
    );
    assert!(err.to_string().contains(&f.smoke.to_string()));
}

#[test]
fn a_rejected_event_in_the_middle_of_a_log_stops_the_fold_there() {
    let f = fernleaf();
    let mut log = f.log.clone();
    // The first driver event in the log, replaced by one its driver cannot accept yet.
    let i = log
        .iter()
        .position(|e| matches!(e.event, ProjectEvent::Driver { .. }))
        .unwrap();
    let ProjectEvent::Driver { driver, .. } = log[i].event else {
        unreachable!()
    };
    log[i].event = ProjectEvent::Driver {
        driver,
        event: DriverEvent::Compacted,
    };
    let err = ProjectState::fold(&log).unwrap_err();
    assert_eq!(err.seq, log[i].seq);
    assert_eq!(err.record, Some(Record::Driver(driver)));
    assert!(matches!(err.cause, Corrupt::Rejected(r) if r.machine == MachineKind::Driver));
}

#[test]
fn events_out_of_order_or_before_the_project_exists_are_errors() {
    let f = fernleaf();
    let mut state = ProjectState::default();
    let err = state.apply(&f.log[1]).unwrap_err();
    assert_eq!(err.cause, Corrupt::OutOfOrder { expected: Seq(1) });

    let mut second_first = f.log[1].clone();
    second_first.seq = Seq(1);
    let err = ProjectState::default().apply(&second_first).unwrap_err();
    assert_eq!(err.cause, Corrupt::NotCreated);

    state.apply(&f.log[0]).unwrap();
    let mut again = f.log[0].clone();
    again.seq = Seq(2);
    assert_eq!(
        state.apply(&again).unwrap_err().cause,
        Corrupt::AlreadyCreated
    );
}

#[test]
fn a_second_driver_acting_for_an_agent_is_an_error() {
    let f = fernleaf();
    let driver = Driver {
        id: DriverId::from_u128(999),
        agent: f.backend,
        harness: opus().harness,
        model: opus().model,
        effort: Effort::High,
        machine: MACHINE,
        purpose: Purpose::Work,
        view: View::Headless,
        why: StartReason::Wake,
    };
    let (mut state, e) = appended(
        &f,
        Actor::Daemon,
        ProjectEvent::DriverStarted(driver.clone()),
    );
    let err = state.apply(&e).unwrap_err();
    assert_eq!(err.cause, Corrupt::SecondDriver(f.backend_driver));

    // A driver outside its agent's range is refused too.
    let low = Driver {
        agent: f.research,
        effort: Effort::Max,
        ..driver
    };
    let (mut state, e) = appended(&f, Actor::Daemon, ProjectEvent::DriverStarted(low));
    let err = state.apply(&e).unwrap_err();
    assert_eq!(
        err.cause,
        Corrupt::OutOfRange {
            effort: Effort::Max,
            range: EffortRange::new(Effort::Low, Effort::High).unwrap(),
        }
    );
}

#[test]
fn a_skill_change_without_a_committed_trial_is_an_error() {
    let f = fernleaf();
    let event = ProjectEvent::SkillCommitted {
        skill: x100_core::testing::deploy_skill(),
        version: SkillVersion(4),
        commit: CommitId("c".into()),
        trial: Some(f.discarded_trial),
    };
    let (mut state, e) = appended(&f, Actor::Daemon, event);
    assert_eq!(
        state.apply(&e).unwrap_err().cause,
        Corrupt::TrialNotCommitted
    );

    let event = ProjectEvent::MemoryCompacted {
        agent: f.tech,
        commit: CommitId("c".into()),
        trial: Some(TrialId::from_u128(404)),
    };
    let (mut state, e) = appended(&f, Actor::Daemon, event);
    assert_eq!(state.apply(&e).unwrap_err().cause, Corrupt::Unknown);
}

#[test]
fn a_direct_skill_change_needs_no_trial_and_a_trial_it_names_must_be_committed() {
    let f = fernleaf();
    let direct = ProjectEvent::SkillCommitted {
        skill: x100_core::testing::deploy_skill(),
        version: SkillVersion(9),
        commit: CommitId("c".into()),
        trial: None,
    };
    let (mut state, e) = appended(&f, Actor::Daemon, direct);
    state.apply(&e).expect("a direct commit is accepted");
    assert_eq!(state.skills().last().unwrap().trial, None);
    let compaction = ProjectEvent::MemoryCompacted {
        agent: f.tech,
        commit: CommitId("c".into()),
        trial: None,
    };
    let (mut state, e) = appended(&f, Actor::Daemon, compaction);
    state.apply(&e).expect("so is a direct compaction");
}

#[test]
fn a_note_read_by_an_agent_that_is_not_on_the_team_is_an_error() {
    let f = fernleaf();
    let read = |agent| ProjectEvent::NoteRead {
        agent,
        note: x100_core::memory::NoteName("auth".into()),
        commit: CommitId("c".into()),
    };
    let (mut state, e) = appended(&f, Actor::Daemon, read(f.tech));
    state.apply(&e).expect("a read by a member");
    let (mut state, e) = appended(
        &f,
        Actor::Daemon,
        read(x100_core::id::AgentId::from_u128(404)),
    );
    assert_eq!(state.apply(&e).unwrap_err().cause, Corrupt::Unknown);
}

#[test]
fn a_change_that_puts_the_leads_over_the_total_is_an_error() {
    let f = fernleaf();
    // Leads hold 18 + 8 + 5 of 50. Raising Product to 28 would make 51.
    let event = ProjectEvent::Agent {
        agent: f.product,
        event: AgentEvent::BudgetSet(Money::from_dollars(28)),
    };
    let (mut state, e) = appended(&f, Actor::Builder, event);
    let err = state.apply(&e).unwrap_err();
    assert_eq!(err.record, Some(Record::Agent(f.product)));
    assert_eq!(
        err.cause,
        Corrupt::Budgets(LeadsOverTotal {
            leads: Money::from_dollars(51),
            total: Money::from_dollars(50),
        })
    );

    // A new lead past the total, as a team change or `form_team` would add it.
    let lead = x100_core::agent::Agent {
        id: AgentId::from_u128(77),
        role: "Ops".into(),
        charter: "Run it.".into(),
        position: Position::Lead {
            effort: EffortRange::FULL,
            budget: Money::from_dollars(20),
        },
        appearance: x100_core::appearance::Appearance::default(),
    };
    let (mut state, e) = appended(&f, Actor::Agent(f.ceo), ProjectEvent::AgentAdded(lead));
    assert!(matches!(
        state.apply(&e).unwrap_err().cause,
        Corrupt::Budgets(_)
    ));

    // The builder lowering the total below what the leads hold.
    let event = ProjectEvent::SettingChanged(x100_core::project::SettingChange::Budget(
        Money::from_dollars(30),
    ));
    let (mut state, e) = appended(&f, Actor::Builder, event);
    assert!(matches!(
        state.apply(&e).unwrap_err().cause,
        Corrupt::Budgets(_)
    ));
}

#[test]
fn a_daemon_event_on_a_record_it_does_not_know_is_an_error() {
    let mut log = daemon_log(Timestamp::from_millis(0));
    let next = Seq(log.len() as u64 + 1);
    log.push(Envelope {
        seq: next,
        at: Timestamp::from_millis(10_000_000),
        by: Actor::Daemon,
        event: DaemonEvent::CoolOffEnded(x100_core::testing::gpt()),
    });
    let err = DaemonState::fold(&log).unwrap_err();
    assert_eq!(err.seq, next);
    assert_eq!(err.record, Some(Record::Harness(x100_core::testing::CODEX)));
    assert_eq!(err.cause, Corrupt::Unknown);
}

#[test]
fn an_agent_goes_back_to_waiting_only_with_a_wait_open() {
    use x100_core::agent::AgentState;
    use x100_core::id::InboxItemId;
    use x100_core::inbox::InboxItem;

    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let agent = state
        .agents()
        .find(|a| a.waits.is_empty() && a.state != AgentState::Retired)
        .map(|a| a.id)
        .expect("an agent with no wait open");
    let woken = ProjectEvent::Agent {
        agent,
        event: AgentEvent::Delivered {
            item: InboxItemId::from_u128(9999),
            what: InboxItem::Task { task: f.smoke },
        },
    };
    let (mut state, e) = appended(&f, Actor::Daemon, woken);
    state.apply(&e).unwrap();
    let back = Envelope {
        seq: Seq(e.seq.0 + 1),
        at: e.at,
        by: Actor::Daemon,
        event: ProjectEvent::Agent {
            agent,
            event: AgentEvent::StillWaiting,
        },
    };
    let err = state.apply(&back).unwrap_err();
    assert_eq!(err.record, Some(Record::Agent(agent)));
    assert_eq!(
        err.cause,
        Corrupt::Rejected(Rejected {
            machine: MachineKind::Agent,
            state: "Working",
            event: "StillWaiting",
        })
    );
    assert_eq!(
        state.agent(agent).unwrap().state,
        AgentState::Working,
        "a refused event changes nothing"
    );
}

#[test]
fn every_agent_has_a_look_by_the_rule_and_the_builder_can_change_one() {
    use x100_core::appearance::{Appearance, Hue, Outline};
    use x100_core::fold::ProjectPart;

    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let look = |a: AgentId| state.agent(a).unwrap().appearance;
    assert_eq!(look(f.ceo), Appearance::default());
    assert_eq!(look(f.ceo).outline, Outline::Circle);
    let leads = [f.tech, f.product, f.marketing].map(look);
    assert!(
        leads
            .iter()
            .all(|l| l.hue.is_none() && l.outline != Outline::Circle)
    );
    let outlines: std::collections::BTreeSet<_> = leads.iter().map(|l| l.outline).collect();
    assert_eq!(outlines.len(), 3, "{leads:?}");
    for (worker, parent) in [(f.backend, f.tech), (f.migrations, f.backend)] {
        assert_ne!(look(worker).outline, look(parent).outline);
    }

    let teal_shield = Appearance {
        outline: Outline::Shield,
        hue: Some(Hue::Teal),
    };
    let set = ProjectEvent::Agent {
        agent: f.tech,
        event: AgentEvent::AppearanceSet(teal_shield),
    };
    let (mut state, e) = appended(&f, Actor::Builder, set);
    let changes = state.apply(&e).unwrap();
    assert_eq!(state.agent(f.tech).unwrap().appearance, teal_shield);
    assert_eq!(state.agent(f.product).unwrap().appearance, leads[1]);
    assert!(changes.parts.contains(&ProjectPart::Looks));
    assert!(changes.parts.contains(&ProjectPart::Team));
    assert!(changes.records.contains(&Record::Agent(f.tech)));
    assert!(!changes.parts.contains(&ProjectPart::Budget));
}
