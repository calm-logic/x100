use x100_core::driver::Incompatible;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, HarnessId, MachineId};
use x100_orch::{Choice, DriverChooser};
use x100_store::DaemonLog;

/// M5's choice: the first allowed model of the first harness, at the lowest effort the agent's
/// range and the model share. The router (M8) replaces it.
pub struct FirstAllowed {
    pub log: DaemonLog,
    pub machine: MachineId,
    /// Harnesses this run can start. The log remembers ones that are gone.
    pub available: Vec<HarnessId>,
}

impl DriverChooser for FirstAllowed {
    fn choose(&self, project: &ProjectState, agent: AgentId) -> Result<Choice, Vec<Incompatible>> {
        let Some(p) = project.project() else {
            return Err(vec![Incompatible::NothingAllowed]);
        };
        let Some(range) = p.team.effort(agent) else {
            return Err(vec![Incompatible::NoEffortInRange]);
        };
        let mut any = false;
        for allowed in &p.settings.allowed {
            if !self.available.contains(&allowed.harness) {
                continue;
            }
            any = true;
            let effort = self.log.with_state(|d| {
                d.harness(allowed.harness)?
                    .models
                    .iter()
                    .find(|m| m.model == allowed.model)?
                    .levels
                    .iter()
                    .map(|(e, _)| *e)
                    .filter(|e| range.contains(*e))
                    .min()
            });
            if let Some(effort) = effort {
                return Ok(Choice {
                    harness: allowed.harness,
                    model: allowed.model.clone(),
                    effort,
                    machine: self.machine,
                });
            }
        }
        Err(vec![if any {
            Incompatible::NoEffortInRange
        } else {
            Incompatible::NothingAllowed
        }])
    }
}
