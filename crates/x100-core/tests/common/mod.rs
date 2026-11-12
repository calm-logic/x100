//! A table check shared by the transition tests.

use std::fmt::Debug;

use x100_core::{MachineKind, Rejected};

/// Checks a machine against a table of the transitions it accepts. Every state is tried with every
/// event: a pair in the table must give the table's next state, and any other pair must be
/// rejected, naming the machine and the state. So the table is the whole machine, as far as these
/// samples reach.
pub fn check_table<S, E>(
    machine: MachineKind,
    next: impl Fn(&S, &E) -> Result<S, Rejected>,
    state_name: impl Fn(&S) -> &'static str,
    states: &[S],
    events: &[E],
    accepted: &[(S, E, S)],
) where
    S: PartialEq + Debug,
    E: PartialEq + Debug,
{
    for (from, event, _) in accepted {
        assert!(
            states.contains(from),
            "table row starts from an unsampled state {from:?}"
        );
        assert!(
            events.contains(event),
            "table row uses an unsampled event {event:?}"
        );
    }
    let mut seen_accepts = 0;
    for state in states {
        for event in events {
            let got = next(state, event);
            match accepted.iter().find(|(s, e, _)| s == state && e == event) {
                Some((_, _, to)) => {
                    assert_eq!(got.as_ref(), Ok(to), "{state:?} + {event:?}");
                    seen_accepts += 1;
                }
                None => {
                    let err = got.expect_err(&format!("{state:?} should reject {event:?}"));
                    assert_eq!(err.machine, machine);
                    assert_eq!(err.state, state_name(state));
                }
            }
        }
    }
    assert_eq!(seen_accepts, accepted.len(), "a table row is listed twice");
}
