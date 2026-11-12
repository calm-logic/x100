//! Where the gate attaches. Every daemon tool call, every merge, every request out through the
//! proxy and every command a harness's permission hook stops is an [`Action`], classified by the
//! rules of `x100-gate` (and its classifier, for what they do not cover) and either allowed or
//! held for the builder by the project's "Ask me about" level.
//!
//! Tool calls and merges are always settled by the rules, so the desk rules on them under the
//! project's lock. Egress and commands may need the classifier, a one-off model call, which
//! [`crate::admit`] makes outside the lock.

use x100_core::action::{Action, ActionEvent, Request};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{ActionId, AgentId};
use x100_core::party::Actor;
use x100_gate::{Ruling, Verdict};

use crate::ctx::{Events, git_of};

/// The rules' ruling on an action they always settle: a tool call or a merge. Should one ever
/// need the classifier, it is held, since an action is never let through for want of an answer.
pub(crate) fn rule(state: &ProjectState, action: &Action) -> Ruling {
    let workspace = workspace(state, action.agent);
    match x100_gate::assess(state, action, workspace.as_deref()) {
        Verdict::Ruled(ruling) => ruling,
        Verdict::Ask(pending) => pending.resolve(None),
    }
}

/// The agent's own worktree, as the path its commands name it by, so a delete inside it is not
/// flagged. `None` before the project has code.
pub(crate) fn workspace(state: &ProjectState, agent: AgentId) -> Option<String> {
    git_of(state).map(|g| g.worktree_path(agent).display().to_string())
}

/// The action proposed and the gate's ruling on it, as the log records them.
pub(crate) fn proposed(action: &Action, ruling: &Ruling) -> Events {
    let mut events = vec![proposal(action)];
    events.extend(ruled(action.id, ruling));
    events
}

pub(crate) fn proposal(action: &Action) -> (Actor, ProjectEvent) {
    let by = match action.request {
        Request::Merge { .. } => Actor::Daemon,
        _ => Actor::Agent(action.agent),
    };
    (by, ProjectEvent::ActionProposed(action.clone()))
}

/// The classification and the decision to allow or hold.
pub(crate) fn ruled(action: ActionId, ruling: &Ruling) -> Events {
    let event = |event| (Actor::Daemon, ProjectEvent::Action { action, event });
    vec![
        event(ActionEvent::Classified(ruling.classification().clone())),
        event(if ruling.is_held() {
            ActionEvent::Held
        } else {
            ActionEvent::Allowed
        }),
    ]
}

pub(crate) fn finished(action: ActionId, error: Option<String>) -> (Actor, ProjectEvent) {
    let event = match error {
        None => ActionEvent::CarriedOut,
        Some(error) => ActionEvent::Failed { error },
    };
    (Actor::Daemon, ProjectEvent::Action { action, event })
}

/// What the agent reads when its action is held, whatever carried it: a tool's reply, a
/// permission hook's denial, or the proxy's refusal. `again` says what an Approve lets it do.
pub(crate) fn held_words(action: ActionId, why: &str, again: &str) -> String {
    format!(
        "x100 is holding this for the builder's decision, as action {action}: {why} Nothing was \
         done. You may wait on the decision on action {action}, or carry on with other work; the \
         decision reaches you either way. {again}"
    )
}

/// What Approve does for each kind of request, told to the agent while it is held.
pub(crate) fn on_approve(request: &Request) -> &'static str {
    match request {
        Request::Tool { .. } => {
            "If the builder approves, x100 carries the call out for you and tells you its result."
        }
        Request::Command { .. } => {
            "If the builder approves, run exactly the same command once more and it is allowed, \
             once."
        }
        Request::Egress { .. } => {
            "If the builder approves, send exactly the same request once more and it goes \
             through, once."
        }
        Request::Merge { .. } => "If the builder approves, the merge is done.",
    }
}
