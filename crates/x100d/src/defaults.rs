//! What a new project starts with, kept in one place: what it may run on, the CEO's effort range,
//! and the budget.
//!
//! Out of the box, work runs on the builder's own local logins, on the cheapest model of each
//! terminal harness: Claude Code's and Codex's terminal programs, each at its cheapest listed
//! price. The router then gives every agent, the CEO included, the cheapest of those, at the
//! lowest effort its task allows. Headless harnesses need an API key, which reaches a driver only
//! through the vault (M11), so on real harnesses none is allowed until then. Fake harnesses cost
//! nothing, so all their models are allowed.

use std::collections::BTreeSet;

use x100_core::action::AskMeAbout;
use x100_core::effort::{Effort, EffortRange};
use x100_core::fold::DaemonState;
use x100_core::harness::{Harness, HarnessModel, ModelOffer};
use x100_core::money::{Money, Percent};
use x100_core::project::Settings;

/// The CEO plans the project and reviews its leads' work, which medium effort does well enough
/// for a first project, and the builder's task, asked at High, runs at Medium. The builder can
/// widen it.
pub fn ceo_effort() -> EffortRange {
    EffortRange::new(Effort::Low, Effort::Medium).expect("low is below medium")
}

/// A new project's settings: ask about high risk only, fifty dollars at list price, a tenth of it
/// for the lab, and the models above.
pub fn settings(daemon: &DaemonState, scripted: bool) -> Settings {
    Settings {
        ask_me_about: AskMeAbout::HighRiskOnly,
        budget: Money::from_dollars(50),
        lab_share: Percent::new(10).expect("a percentage"),
        allowed: allowed(daemon, scripted),
    }
}

/// Of the harnesses a machine has now: a harness the log remembers from an earlier scan, such as a
/// terminal harness whose login has gone, is not offered.
fn allowed(daemon: &DaemonState, scripted: bool) -> BTreeSet<HarnessModel> {
    let found = daemon.machines().flat_map(|m| m.harnesses.iter());
    let mut out = BTreeSet::new();
    for h in found.filter_map(|id| daemon.harness(*id)) {
        let models: Vec<&ModelOffer> = match (scripted, &h.login) {
            (true, _) => h.models.iter().collect(),
            (false, Some(_)) => cheapest(h).into_iter().collect(),
            (false, None) => Vec::new(),
        };
        out.extend(models.into_iter().map(|m| HarnessModel {
            harness: h.id,
            model: m.model.clone(),
        }));
    }
    out
}

/// The model whose cheapest level is cheapest, by the router's measure.
fn cheapest(h: &Harness) -> Option<&ModelOffer> {
    h.models.iter().min_by_key(|m| {
        m.levels
            .iter()
            .map(|(_, p)| x100_router::blended(p))
            .min()
            .unwrap_or(u128::MAX)
    })
}
