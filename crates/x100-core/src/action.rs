//! Actions: what an agent wants to do, with its classification and decision. Every daemon tool call
//! is an action, and so are commands a harness's permission hook stops, network egress, and branch
//! merges.

use serde::{Deserialize, Serialize};

use crate::id::{ActionId, AgentId, DriverId, OneOffCallId, TaskId};
use crate::machine::{MachineKind, Rejected};
use crate::tool::ToolCall;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Action {
    pub id: ActionId,
    /// The agent that wants it done, or for a merge, the agent whose acceptance causes it.
    pub agent: AgentId,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// A call to one of the daemon's tools, from the driver at whose address it arrived.
    Tool { driver: DriverId, call: ToolCall },
    /// A command a harness's permission hook stopped. On Approve it is allowed once, for this
    /// agent.
    Command { driver: DriverId, command: String },
    /// A request out of the sandbox through the proxy.
    Egress {
        driver: DriverId,
        host: String,
        method: String,
    },
    /// Merging the assignee's branch into the delegator's when a task is accepted, or the CEO's
    /// into main.
    Merge { task: TaskId },
}

/// The kinds the gate's rules know. Anything else is `Other`, which the classifier labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    DeleteData,
    DeployToProduction,
    Spend,
    SendOutside,
    ChangeCredentials,
    ForcePush,
    /// Any change to the team after the first. Always held in v1.
    TeamChange,
    Merge,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Classification {
    pub kind: ActionKind,
    pub risk: Risk,
    pub by: ClassifiedBy,
    /// Why, in a sentence the builder sees on a held action.
    pub why: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifiedBy {
    Rules,
    Classifier(OneOffCallId),
}

/// The builder's "Ask me about" setting: which actions wait for the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskMeAbout {
    Nothing,
    HighRiskOnly,
    MediumAndUp,
    EveryDecision,
}

impl AskMeAbout {
    /// Whether an action at `risk` waits for the builder at this setting.
    pub fn holds(self, risk: Risk) -> bool {
        match self {
            AskMeAbout::Nothing => false,
            AskMeAbout::HighRiskOnly => risk == Risk::High,
            AskMeAbout::MediumAndUp => risk >= Risk::Medium,
            AskMeAbout::EveryDecision => true,
        }
    }
}

/// The builder's answer to a held action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionState {
    Proposed,
    Classified,
    Allowed,
    Held,
    Approved,
    Denied,
    CarriedOut,
    Failed,
}

impl ActionState {
    pub const INITIAL: ActionState = ActionState::Proposed;

    pub fn name(self) -> &'static str {
        match self {
            ActionState::Proposed => "Proposed",
            ActionState::Classified => "Classified",
            ActionState::Allowed => "Allowed",
            ActionState::Held => "Held",
            ActionState::Approved => "Approved",
            ActionState::Denied => "Denied",
            ActionState::CarriedOut => "CarriedOut",
            ActionState::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionEvent {
    Classified(Classification),
    Allowed,
    Held,
    Approved,
    Denied,
    CarriedOut,
    Failed { error: String },
}

impl ActionEvent {
    pub fn name(&self) -> &'static str {
        match self {
            ActionEvent::Classified(_) => "Classified",
            ActionEvent::Allowed => "Allowed",
            ActionEvent::Held => "Held",
            ActionEvent::Approved => "Approved",
            ActionEvent::Denied => "Denied",
            ActionEvent::CarriedOut => "CarriedOut",
            ActionEvent::Failed { .. } => "Failed",
        }
    }
}

/// The action machine. Proposed → Classified → Allowed, or Held → Approved or Denied. Allowed or
/// Approved → Carried out or Failed. Only an Allowed or Approved action is ever carried out.
pub fn next(state: &ActionState, event: &ActionEvent) -> Result<ActionState, Rejected> {
    use ActionEvent as E;
    use ActionState as S;
    let next = match (state, event) {
        (S::Proposed, E::Classified(_)) => Some(S::Classified),
        (S::Classified, E::Allowed) => Some(S::Allowed),
        (S::Classified, E::Held) => Some(S::Held),
        (S::Held, E::Approved) => Some(S::Approved),
        (S::Held, E::Denied) => Some(S::Denied),
        (S::Allowed | S::Approved, E::CarriedOut) => Some(S::CarriedOut),
        (S::Allowed | S::Approved, E::Failed { .. }) => Some(S::Failed),
        _ => None,
    };
    next.ok_or(Rejected {
        machine: MachineKind::Action,
        state: state.name(),
        event: event.name(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_me_about_holds_what_the_setting_names() {
        use AskMeAbout::*;
        use Risk::*;
        let held = |level: AskMeAbout| [Low, Medium, High].map(|r| level.holds(r));
        assert_eq!(held(Nothing), [false, false, false]);
        assert_eq!(held(HighRiskOnly), [false, false, true]);
        assert_eq!(held(MediumAndUp), [false, true, true]);
        assert_eq!(held(EveryDecision), [true, true, true]);
    }
}
