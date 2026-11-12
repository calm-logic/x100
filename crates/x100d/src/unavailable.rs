//! The methods a later milestone makes real. They all answer from here, so a milestone that
//! builds one deletes its line and nothing else.

use x100_proto::{Call, ErrorCode, RpcError};

/// The milestone that makes this method real, or `None` if it is real already.
pub fn arrives_with(call: &Call) -> Option<&'static str> {
    Some(match call {
        Call::ReplaceDriver(_) => "M9",
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
