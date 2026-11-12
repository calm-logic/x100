//! Where the sandbox attaches. Every driver an agent has runs in the agent's one sandbox, which M10
//! builds behind [`Sandbox`] by wrapping the driver's command line. Until then a sandbox is a plain
//! worktree and [`NoSandbox`] leaves the command as it is.

use std::path::Path;

use x100_core::id::AgentId;
use x100_drivers::CommandWrapper;

pub trait Sandbox: Send + Sync {
    /// How `agent`'s driver, working in `worktree`, is run.
    fn wrap(&self, agent: AgentId, worktree: &Path) -> CommandWrapper;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoSandbox;

impl Sandbox for NoSandbox {
    fn wrap(&self, _: AgentId, _: &Path) -> CommandWrapper {
        CommandWrapper::identity()
    }
}
