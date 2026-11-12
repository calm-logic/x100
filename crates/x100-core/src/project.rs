//! A project and its settings.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::action::AskMeAbout;
use crate::harness::HarnessModel;
use crate::money::{Money, Percent};

/// What a project is called and what it is for. A new project has neither: its CEO sets them with
/// `set_up_project` once the builder has said enough, and the CEO or the builder may change them
/// later.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    /// One line saying what the project is for.
    pub intent: String,
}

/// A project's settings. Lead budgets and effort ranges show in Settings but live on the agents.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Settings {
    pub ask_me_about: AskMeAbout,
    /// The most the project may spend on models, in total.
    pub budget: Money,
    /// The lab's cap, as a share of the project's budget.
    pub lab_share: Percent,
    /// Harnesses and models the builder allows in this project.
    pub allowed: BTreeSet<HarnessModel>,
}

impl Settings {
    pub fn lab_cap(&self) -> Money {
        self.lab_share.of(self.budget)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingChange {
    AskMeAbout(AskMeAbout),
    Budget(Money),
    LabShare(Percent),
    /// Turning one off hands the work of any driver using it to another driver.
    Allow {
        what: HarnessModel,
        allowed: bool,
    },
}
