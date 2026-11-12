use x100_core::driver::Incompatible;
use x100_core::effort::Effort;
use x100_core::fold::ProjectState;
use x100_core::harness::ModelName;
use x100_core::id::{AgentId, HarnessId, MachineId};

/// A driver to start: the harness, model and effort, and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub harness: HarnessId,
    pub model: ModelName,
    pub effort: Effort,
    pub machine: MachineId,
}

/// Asked whenever an agent needs a driver. The router (M8) implements it with efforts, prices,
/// cool-offs and budgets; until then the daemon takes the first allowed model of the first harness.
pub trait DriverChooser: Send + Sync {
    /// `Err` says why no driver fits, and the agent then waits for one.
    fn choose(&self, project: &ProjectState, agent: AgentId) -> Result<Choice, Vec<Incompatible>>;
}
