//! Exhaustive model checking of the state machines with Stateright.
//!
//! Every model drives the crate's real transition functions, passed in as plain function values.
//! Each model also has broken variants: the real function wrapped with one named bug. A test runs
//! the checker on each broken variant and asserts that it finds a counterexample to the invariant
//! the bug breaks, so a passing proof is known to be able to fail.
//!
//! Stateright's `eventually` properties miss violations on paths that end in a cycle, so liveness
//! is stated as a safety property over terminal states instead: "every state with no successor
//! satisfies X".

mod action;
mod budget;
mod job;
mod takeover;
mod team;
mod trial;
mod wait;

use std::fmt::Debug;
use std::hash::Hash;

use stateright::{Checker, HasDiscoveries, Model};

/// Whether no action leads anywhere from `state`.
pub(crate) fn is_terminal<M: Model>(model: &M, state: &M::State) -> bool {
    let mut actions = Vec::new();
    model.actions(state, &mut actions);
    actions
        .into_iter()
        .all(|a| model.next_state(state, a).is_none())
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Explores every reachable state of `model` and asserts that every property holds: each `always`
/// in every state, and each `sometimes` in at least one, so no proof passes by reaching nothing.
pub(crate) fn prove<M>(name: &str, model: M) -> usize
where
    M: Model + Send + Sync + 'static,
    M::State: Clone + Debug + Hash + PartialEq + Send + Sync,
    M::Action: Clone + Debug + PartialEq + Send + Sync,
{
    let checker = model.checker().threads(threads()).spawn_bfs().join();
    checker.assert_properties();
    let unique = checker.unique_state_count();
    println!(
        "proved {name}: {unique} unique states ({} visited), max depth {}, every property holds",
        checker.state_count(),
        checker.max_depth()
    );
    unique
}

/// Runs the checker on a model with a bug and asserts that it finds a counterexample to `property`.
/// It stops at the first one.
pub(crate) fn refute<M>(name: &str, model: M, property: &'static str) -> usize
where
    M: Model + Send + Sync + 'static,
    M::State: Clone + Debug + Hash + PartialEq + Send + Sync,
    M::Action: Clone + Debug + PartialEq + Send + Sync,
{
    let checker = model
        .checker()
        .threads(threads())
        .finish_when(HasDiscoveries::AnyOf([property].into()))
        .spawn_bfs()
        .join();
    let actions = checker.assert_any_discovery(property).into_actions();
    println!(
        "refuted {name}: \"{property}\" fails after {} steps: {actions:?}",
        actions.len()
    );
    actions.len()
}
