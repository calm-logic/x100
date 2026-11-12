//! What happens to a held action once the builder answers, for each kind of request.
//!
//! A held action never holds up a driver. The requester is answered at once ([`AtOnce`]), and the
//! builder's answer later does the [`Effect`] below and goes to the agent's inbox
//! ([`tell_agent`]) whichever driver the agent has by then.

use x100_core::action::{Decision, Request};
use x100_core::id::ActionId;
use x100_core::inbox::InboxItem;

/// What the requester is told when its action is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtOnce {
    /// The tool call returns at once, saying the action is held and that the agent may wait on
    /// the decision.
    ToolReplyHeld,
    /// The permission hook is answered with a denial that says the command is held. The hook
    /// pauses the harness's turn while it is unanswered, so it is never left waiting.
    HookDenialSaysHeld,
    /// The proxy refuses the request at once, with a message that says it is held.
    RefuseSaysHeld,
    /// `review` returns. The task stays Promoting, and the merge waits for the decision.
    MergeWaits,
}

/// What the daemon does for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Carry out the held tool call itself, as the agent, and send the result to its inbox. The
    /// driver it arrived from is not consulted: that driver may be gone.
    RunToolCall,
    /// Allow this exact command once for this agent, so its retry passes the hook.
    AllowCommandOnce,
    /// Allow this exact request once for this agent, so its retry passes the proxy.
    AllowEgressOnce,
    /// Do the merge, as if the gate had allowed it.
    MergeBranch,
    /// Nothing runs and nothing is granted. The action ends Denied.
    Drop,
    /// The merge is not done. The task goes back to the assignee with the denial as the reason
    /// (a `MergeFailed` event), so it is not stuck Promoting.
    SendTaskBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume {
    pub at_once: AtOnce,
    pub on_approve: Effect,
    pub on_deny: Effect,
}

pub fn resume(request: &Request) -> Resume {
    match request {
        Request::Tool { .. } => Resume {
            at_once: AtOnce::ToolReplyHeld,
            on_approve: Effect::RunToolCall,
            on_deny: Effect::Drop,
        },
        Request::Command { .. } => Resume {
            at_once: AtOnce::HookDenialSaysHeld,
            on_approve: Effect::AllowCommandOnce,
            on_deny: Effect::Drop,
        },
        Request::Egress { .. } => Resume {
            at_once: AtOnce::RefuseSaysHeld,
            on_approve: Effect::AllowEgressOnce,
            on_deny: Effect::Drop,
        },
        Request::Merge { .. } => Resume {
            at_once: AtOnce::MergeWaits,
            on_approve: Effect::MergeBranch,
            on_deny: Effect::SendTaskBack,
        },
    }
}

impl Resume {
    pub fn effect(&self, decision: Decision) -> Effect {
        match decision {
            Decision::Approved => self.on_approve,
            Decision::Denied => self.on_deny,
        }
    }
}

/// The item for the agent's inbox, for every kind of request and either decision. The agent is
/// told by the inbox and not by a driver, so a decision reaches the agent that replaced it.
pub fn tell_agent(action: ActionId, decision: Decision) -> InboxItem {
    InboxItem::Decision { action, decision }
}
