//! The tools later milestones build. They answer from here, so a milestone that makes one real
//! deletes its line and nothing else.

use x100_core::tool::ToolCall;

/// The milestone that makes this tool real, or `None` if it is real already.
pub(crate) fn arrives_with(call: &ToolCall) -> Option<&'static str> {
    Some(match call {
        ToolCall::StartProcess { .. } | ToolCall::StopProcess { .. } => "M10",
        ToolCall::LoadSkill { .. } | ToolCall::Remember { .. } | ToolCall::ReadMemories { .. } => {
            "M13"
        }
        ToolCall::ReadLog { .. } | ToolCall::ProposeChange(_) => "M14",
        _ => return None,
    })
}
