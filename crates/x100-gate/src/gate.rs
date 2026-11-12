//! The two stages and the decision: rules first, the classifier for what they miss, then the
//! builder's "Ask me about" level.

use x100_core::action::{
    Action, ActionKind, AskMeAbout, Classification, ClassifiedBy, Request, Risk,
};
use x100_core::fold::ProjectState;
use x100_core::id::TaskId;
use x100_core::money::Percent;
use x100_core::party::Party;

use crate::classify::{self, Classifier, Pending, Subject};
use crate::command::{self, Reading};
use crate::egress;
use crate::rule::{self, Matched};
use crate::tools;

/// The gate's answer: carry the action out now, or hold it for the builder. One or the other, by
/// construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ruling {
    Allow(Classification),
    Hold(Classification),
}

impl Ruling {
    pub fn new(level: AskMeAbout, classification: Classification) -> Self {
        if holds(level, &classification) {
            Ruling::Hold(classification)
        } else {
            Ruling::Allow(classification)
        }
    }

    pub fn classification(&self) -> &Classification {
        match self {
            Ruling::Allow(c) | Ruling::Hold(c) => c,
        }
    }

    pub fn is_held(&self) -> bool {
        matches!(self, Ruling::Hold(_))
    }
}

/// What the rules could do with an action.
#[derive(Debug)]
pub enum Verdict {
    Ruled(Ruling),
    /// No rule covers it. The caller asks the decision model (see [`Pending::question`]) and
    /// finishes with [`Pending::resolve`]. If it cannot wait for an answer it resolves with
    /// [`crate::Asked::NoAnswer`], which holds the action.
    Ask(Pending),
}

/// Whether an action with this classification waits for the builder at `level`. A change to the
/// team after the first waits at every level.
pub fn holds(level: AskMeAbout, classification: &Classification) -> bool {
    classification.kind == ActionKind::TeamChange || level.holds(classification.risk)
}

/// The level in force. A project that cannot be read is asked about everything.
pub(crate) fn level(state: &ProjectState) -> AskMeAbout {
    state
        .project()
        .map_or(AskMeAbout::EveryDecision, |p| p.settings.ask_me_about)
}

/// Applies the rules to `action`. `workspace` is the directory of the agent's own sandbox, so that
/// a delete inside it is not flagged.
pub fn assess(state: &ProjectState, action: &Action, workspace: Option<&str>) -> Verdict {
    let level = level(state);
    let ruled = |m: Matched| Verdict::Ruled(Ruling::new(level, from_rule(&m)));
    match &action.request {
        Request::Tool { call, .. } => ruled(tools::rule(state, action.agent, call, workspace)),
        Request::Merge { task } => ruled(merge_rule(state, *task)),
        Request::Command { command, .. } => match command::read(command, workspace) {
            Reading::Matched(m) => ruled(m),
            Reading::Ordinary => ruled(Matched::of(&rule::CMD_ORDINARY)),
            Reading::Unknown => classify::ask(action, Subject::Command(command), Risk::Low, level),
        },
        Request::Egress { host, method, .. } => match egress::read(state, host, method) {
            egress::Reading::Matched(m) => ruled(m),
            egress::Reading::Unknown { floor } => {
                classify::ask(action, Subject::Egress { host, method }, floor, level)
            }
        },
    }
}

/// [`assess`], then the classifier's answer if one is needed, raised a level when its confidence
/// is under `threshold`. A classifier that gives no answer holds the action.
pub fn judge(
    state: &ProjectState,
    action: &Action,
    workspace: Option<&str>,
    classifier: &dyn Classifier,
    threshold: Percent,
) -> Ruling {
    match assess(state, action, workspace) {
        Verdict::Ruled(ruling) => ruling,
        Verdict::Ask(pending) => {
            let asked = classifier.classify(pending.question());
            pending.resolve(asked, threshold)
        }
    }
}

pub(crate) fn from_rule(m: &Matched) -> Classification {
    let why = if m.detail.is_empty() {
        format!("{} [{}]", m.rule.why, m.rule.id)
    } else {
        format!("{} [{}: {}]", m.rule.why, m.rule.id, m.detail)
    };
    Classification {
        kind: m.rule.kind,
        risk: m.rule.risk,
        by: ClassifiedBy::Rules,
        why,
    }
}

/// A merge into main is the builder's product changing; a merge between agents' branches is not.
fn merge_rule(state: &ProjectState, task: TaskId) -> Matched {
    let delegator = state
        .task(task)
        .and_then(|t| t.state.assignment().map(|a| a.delegator));
    match delegator {
        Some(Party::Agent(_)) => Matched::of(&rule::MERGE_BRANCH),
        Some(Party::Builder) => Matched::of(&rule::MERGE_MAIN),
        None => Matched::because(&rule::MERGE_MAIN, "the task is not in the log"),
    }
}
