//! The tools later milestones build. They answer from here and are not offered to agents, so a
//! milestone that makes one real deletes its line and nothing else.

use super::Tool;

/// The milestone that makes this tool real, or `None` if it is real already.
pub(crate) fn arrives_with(tool: Tool) -> Option<&'static str> {
    Some(match tool {
        Tool::LoadSkill | Tool::Remember | Tool::ReadMemories => "M13",
        Tool::ReadLog | Tool::ProposeChange => "M14",
        _ => return None,
    })
}
