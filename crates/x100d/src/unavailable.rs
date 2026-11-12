//! The methods a later milestone makes real. They all answer from here, so a milestone that
//! builds one deletes its line and nothing else.

use x100_proto::{Call, ErrorCode, RpcError};

/// The milestone that makes this method real, or `None` if it is real already.
pub fn arrives_with(call: &Call) -> Option<&'static str> {
    Some(match call {
        Call::ReadWaits(_) | Call::ReadStartPack(_) => "M6",
        Call::SetMachineCap(_)
        | Call::RescanHarnesses(_)
        | Call::SetLeadBudget(_)
        | Call::SetEffortRange(_) => "M8",
        Call::ReplaceDriver(_) => "M9",
        Call::AddCredential(_)
        | Call::ReplaceCredential(_)
        | Call::ScopeCredential(_)
        | Call::RemoveCredential(_) => "M11",
        Call::DecideAction(_) => "M12",
        Call::ListMemories(_) | Call::ListSkills(_) => "M13",
        Call::Reflect(_) | Call::ListTrials(_) => "M14",
        _ => return None,
    })
}

pub fn not_yet(call: &Call) -> Option<RpcError> {
    arrives_with(call).map(|milestone| {
        RpcError::new(
            ErrorCode::Unavailable,
            format!(
                "{} is not available yet: it arrives with {milestone}",
                call.method()
            ),
        )
    })
}
