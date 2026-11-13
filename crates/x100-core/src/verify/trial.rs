//! "No change from reflection reaches an agent's skills or memories without a Committed trial
//! behind it."
//!
//! In the log a skill change or memory compaction is `SkillCommitted` or `MemoryCompacted`. One
//! that names a trial needs it Committed, which the fold checks. One that names none was
//! committed directly, as the project's "trial before commit" setting, off by default, allows.
//! The model covers the gated path: whether a trial can be committed without the evidence. The
//! environment tries every trial event in every state; a change reaches the main
//! branch exactly when the machine accepts `Committed`.

use std::time::Duration;

use stateright::{Model, Property};

use super::{prove, refute};
use crate::Rejected;
use crate::git::CommitId;
use crate::money::Money;
use crate::trial::{self, Arm, RunScore, TrialEvent, TrialState, Verdict};

type Next = fn(&TrialState, &TrialEvent) -> Result<TrialState, Rejected>;

struct Trials {
    next: Next,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct State {
    trial: TrialState,
    runs: u8,
    scored_as_supporting: bool,
    reached_main: bool,
}

const RUNS: u8 = 2;

fn events() -> Vec<TrialEvent> {
    vec![
        TrialEvent::Started,
        TrialEvent::RunFinished(RunScore {
            arm: Arm::Changed,
            run: 1,
            checks_passed: 1,
            checks_total: 1,
            tool_errors: 0,
            turns: 1,
            duration: Duration::from_secs(1),
            cost: Money::from_micros(1),
            judged: None,
        }),
        TrialEvent::Scored(Verdict::Supports),
        TrialEvent::Scored(Verdict::DoesNotSupport),
        TrialEvent::Committed {
            commit: CommitId(String::new()),
        },
        TrialEvent::Discarded { why: String::new() },
        TrialEvent::Stopped { why: String::new() },
    ]
}

impl Model for Trials {
    type State = State;
    type Action = TrialEvent;

    fn init_states(&self) -> Vec<State> {
        vec![State {
            trial: TrialState::INITIAL,
            runs: 0,
            scored_as_supporting: false,
            reached_main: false,
        }]
    }

    fn actions(&self, s: &State, actions: &mut Vec<TrialEvent>) {
        actions.extend(
            events()
                .into_iter()
                .filter(|e| s.runs < RUNS || !matches!(e, TrialEvent::RunFinished(_))),
        );
    }

    fn next_state(&self, s: &State, event: TrialEvent) -> Option<State> {
        let trial = (self.next)(&s.trial, &event).ok()?;
        Some(State {
            trial,
            runs: s.runs + u8::from(matches!(event, TrialEvent::RunFinished(_))),
            scored_as_supporting: s.scored_as_supporting
                || trial == TrialState::Scored(Verdict::Supports),
            reached_main: s.reached_main || matches!(event, TrialEvent::Committed { .. }),
        })
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always(
                "a change reaches the main branch only behind a trial that supports it",
                |_, s: &State| {
                    !s.reached_main || (s.scored_as_supporting && s.trial == TrialState::Committed)
                },
            ),
            Property::always(
                "a stopped or discarded trial changes nothing",
                |_, s: &State| {
                    !matches!(s.trial, TrialState::Stopped | TrialState::Discarded)
                        || !s.reached_main
                },
            ),
            Property::sometimes("a trial is committed", |_, s: &State| {
                s.trial == TrialState::Committed
            }),
            Property::sometimes("a trial is discarded", |_, s: &State| {
                s.trial == TrialState::Discarded
            }),
            Property::sometimes("a trial is stopped at the cap", |_, s: &State| {
                s.trial == TrialState::Stopped
            }),
        ]
    }
}

/// Bug: a trial whose verdict does not support its change is committed anyway.
fn commits_without_support(s: &TrialState, e: &TrialEvent) -> Result<TrialState, Rejected> {
    match (s, e) {
        (TrialState::Scored(Verdict::DoesNotSupport), TrialEvent::Committed { .. }) => {
            Ok(TrialState::Committed)
        }
        _ => trial::next(s, e),
    }
}

#[test]
fn no_change_reaches_main_without_a_supporting_trial() {
    prove("trial", Trials { next: trial::next });
}

#[test]
fn a_change_committed_without_support_is_caught() {
    refute(
        "trial with bug commits_without_support",
        Trials {
            next: commits_without_support,
        },
        "a change reaches the main branch only behind a trial that supports it",
    );
}
