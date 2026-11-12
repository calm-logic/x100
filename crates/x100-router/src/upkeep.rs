//! Warm drivers between turns: how long one stays warm, when it is compacted, and when it ends.
//!
//! A driver whose turn ends stays warm for its model's prompt cache lifetime, so a task that
//! arrives meanwhile finds the cache. Between tasks it is retired once its context passes the
//! daemon's share of the window, once it has carried the daemon's number of tasks, or once it is
//! older than the daemon's age. After a task it carried is accepted it is compacted before its next
//! turn if its harness can, and retired if not, which [`crate::choose`] decides. Retiring needs no
//! last turn: an idle agent has reported its task and a
//! waiting one wrote its progress note when it called `wait`, and the next driver starts from a
//! start pack that carries both.

use std::time::Duration;

use x100_core::driver::{Driver, RetireReason};
use x100_core::fold::{DaemonState, DriverRecord};
use x100_core::time::Timestamp;

use crate::choose::{Warm, unfit};
use crate::rules::{Ask, Candidate};
use crate::world::World;

/// How long a driver stays warm when its harness no longer lists its model.
pub const DEFAULT_WARM: Duration = Duration::from_secs(300);

/// What to do with a warm driver now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upkeep {
    Keep,
    End(RetireReason),
}

/// When `driver`'s warm period ends if its turn ends at `now`: its model's cache lifetime later.
pub fn warm_until(daemon: &DaemonState, driver: &Driver, now: Timestamp) -> Timestamp {
    let lifetime = daemon
        .harness(driver.harness)
        .and_then(|h| h.models.iter().find(|m| m.model == driver.model))
        .map_or(DEFAULT_WARM, |m| m.cache_lifetime);
    now + lifetime
}

/// What becomes of a driver between turns: asked as its turn ends, or of a warm one when the
/// project's settings change. A driver that has already ended is kept, which changes nothing.
pub fn upkeep(world: &World, warm: &Warm) -> Upkeep {
    let Some(rec) = world.project.driver(warm.driver) else {
        return Upkeep::Keep;
    };
    if !rec.state.is_live() {
        return Upkeep::Keep;
    }
    let d = &rec.driver;
    let Some(project) = world.project.project() else {
        return Upkeep::Keep;
    };
    let Some(range) = project.team.effort(d.agent) else {
        return Upkeep::Keep;
    };
    let Some(payer) = project.team.payer(d.agent) else {
        return Upkeep::Keep;
    };
    let ask = Ask::new(*world, project, range, range.min(), payer);
    let c = Candidate {
        harness: d.harness,
        model: &d.model,
        effort: d.effort,
        price: None,
        machine: d.machine,
        login: None,
        holds_slot: true,
    };
    if let Err(why) = unfit(&ask, &c) {
        return Upkeep::End(why);
    }
    let between_tasks = world
        .project
        .agent(d.agent)
        .is_some_and(|a| a.open_tasks.is_empty());
    match worn(world, rec, warm).filter(|_| between_tasks) {
        Some(reason) => Upkeep::End(reason),
        None => Upkeep::Keep,
    }
}

/// The daemon's thresholds for retiring a driver, the first one it has passed.
pub(crate) fn worn(world: &World, rec: &DriverRecord, warm: &Warm) -> Option<RetireReason> {
    let settings = world.daemon.settings();
    let full = rec.context.is_some_and(|fill| {
        u128::from(fill.used) * 100
            > u128::from(fill.window) * u128::from(settings.retire_at_context.value())
    });
    if full {
        return Some(RetireReason::ContextFull);
    }
    if warm.carried.len() as u64 >= u64::from(settings.retire_after_tasks) {
        return Some(RetireReason::TooManyTasks);
    }
    if world.now.since(rec.started) >= settings.retire_after_age {
        return Some(RetireReason::TooOld);
    }
    None
}
