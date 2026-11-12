//! Where the gate attaches. Every daemon tool call and every merge is an [`Action`], and before
//! one is carried out it is classified and either allowed or held. M12 puts the rules, the
//! classifier and the builder's "Ask me about" level behind [`Gate`]; until then [`AllowAll`]
//! classifies everything as routine and allows it.

use x100_core::action::{
    Action, ActionEvent, ActionKind, Classification, ClassifiedBy, Request, Risk,
};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::party::Actor;

/// What the gate decided about one action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    pub classification: Classification,
    /// Held for the builder's Approve or Deny instead of carried out now.
    pub hold: bool,
}

pub trait Gate: Send + Sync {
    fn rule(&self, project: &ProjectState, action: &Action) -> Ruling;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAll;

impl Gate for AllowAll {
    fn rule(&self, _: &ProjectState, action: &Action) -> Ruling {
        let kind = match action.request {
            Request::Merge { .. } => ActionKind::Merge,
            _ => ActionKind::Other,
        };
        Ruling {
            classification: Classification {
                kind,
                risk: Risk::Low,
                by: ClassifiedBy::Rules,
                why: "Allowed: nothing is held until the gate arrives with M12.".into(),
            },
            hold: false,
        }
    }
}

/// The action proposed and the gate's ruling on it, as the log records them.
pub(crate) fn proposed(action: &Action, ruling: &Ruling) -> Vec<(Actor, ProjectEvent)> {
    let by = match action.request {
        Request::Merge { .. } => Actor::Daemon,
        _ => Actor::Agent(action.agent),
    };
    let id = action.id;
    let event = |event| (Actor::Daemon, ProjectEvent::Action { action: id, event });
    vec![
        (by, ProjectEvent::ActionProposed(action.clone())),
        event(ActionEvent::Classified(ruling.classification.clone())),
        event(if ruling.hold {
            ActionEvent::Held
        } else {
            ActionEvent::Allowed
        }),
    ]
}

pub(crate) fn finished(
    action: x100_core::id::ActionId,
    error: Option<String>,
) -> (Actor, ProjectEvent) {
    let event = match error {
        None => ActionEvent::CarriedOut,
        Some(error) => ActionEvent::Failed { error },
    };
    (Actor::Daemon, ProjectEvent::Action { action, event })
}
