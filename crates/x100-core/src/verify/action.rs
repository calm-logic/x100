//! "A Held action never runs before it is Approved", over every sequence of gate events.

use stateright::{Model, Property};

use super::{prove, refute};
use crate::Rejected;
use crate::action::{
    self, ActionEvent, ActionKind, ActionState, Classification, ClassifiedBy, Risk,
};

type Next = fn(&ActionState, &ActionEvent) -> Result<ActionState, Rejected>;

/// The environment proposes every gate event in every state; the machine decides which take effect.
struct Gate {
    next: Next,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct State {
    action: ActionState,
    was_held: bool,
    was_approved: bool,
    /// The state the action was in when it was carried out, if it was.
    carried_out_from: Option<ActionState>,
}

fn events() -> Vec<ActionEvent> {
    vec![
        ActionEvent::Classified(Classification {
            kind: ActionKind::DeleteData,
            risk: Risk::High,
            by: ClassifiedBy::Rules,
            why: String::new(),
        }),
        ActionEvent::Allowed,
        ActionEvent::Held,
        ActionEvent::Approved,
        ActionEvent::Denied,
        ActionEvent::CarriedOut,
        ActionEvent::Failed {
            error: String::new(),
        },
    ]
}

impl Model for Gate {
    type State = State;
    type Action = ActionEvent;

    fn init_states(&self) -> Vec<State> {
        vec![State {
            action: ActionState::INITIAL,
            was_held: false,
            was_approved: false,
            carried_out_from: None,
        }]
    }

    fn actions(&self, _: &State, actions: &mut Vec<ActionEvent>) {
        actions.extend(events());
    }

    fn next_state(&self, s: &State, event: ActionEvent) -> Option<State> {
        let action = (self.next)(&s.action, &event).ok()?;
        Some(State {
            action,
            was_held: s.was_held || action == ActionState::Held,
            was_approved: s.was_approved || action == ActionState::Approved,
            carried_out_from: if action == ActionState::CarriedOut
                && s.action != ActionState::CarriedOut
            {
                Some(s.action)
            } else {
                s.carried_out_from
            },
        })
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always(
                "a held action never runs before it is approved",
                |_, s: &State| !(s.carried_out_from.is_some() && s.was_held && !s.was_approved),
            ),
            Property::always(
                "only an allowed or approved action is carried out",
                |_, s: &State| {
                    matches!(
                        s.carried_out_from,
                        None | Some(ActionState::Allowed | ActionState::Approved)
                    )
                },
            ),
            Property::sometimes(
                "a held action is approved and carried out",
                |_, s: &State| s.was_held && s.action == ActionState::CarriedOut,
            ),
            Property::sometimes("an allowed action is carried out", |_, s: &State| {
                !s.was_held && s.action == ActionState::CarriedOut
            }),
        ]
    }
}

/// Bug: a held action is carried out without waiting for the builder.
fn runs_held_action(s: &ActionState, e: &ActionEvent) -> Result<ActionState, Rejected> {
    match (s, e) {
        (ActionState::Held, ActionEvent::CarriedOut) => Ok(ActionState::CarriedOut),
        _ => action::next(s, e),
    }
}

#[test]
fn a_held_action_never_runs_before_it_is_approved() {
    prove("action", Gate { next: action::next });
}

#[test]
fn a_held_action_run_without_approval_is_caught() {
    refute(
        "action with bug runs_held_action",
        Gate {
            next: runs_held_action,
        },
        "a held action never runs before it is approved",
    );
}
