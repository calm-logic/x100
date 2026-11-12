//! `start_process` and `stop_process`: a long-running command owned by the agent's sandbox, so it
//! outlives the driver that started it. The decision is here; the desk starts or stops the process
//! itself before the events are appended.

use x100_core::event::ProjectEvent;
use x100_core::party::Actor;
use x100_core::process::{Process, ProcessEnd, ProcessName};

use super::{At, Plan, Refusal};

/// What the desk does in the agent's sandbox for a call that passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessPlan {
    Start { name: ProcessName, command: String },
    Stop { name: ProcessName },
}

/// The longest name, which names the file its output goes to.
const NAME_MAX: usize = 64;

pub(crate) fn start(at: &At, name: &ProcessName, command: &str) -> Result<Plan, Refusal> {
    let ok = !name.0.is_empty()
        && name.0.len() <= NAME_MAX
        && !name.0.starts_with('.')
        && name
            .0
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        return Err(format!(
            "a process's name is 1 to {NAME_MAX} letters, digits, `-`, `_` or `.`, not starting \
             with `.`"
        ));
    }
    if command.trim().is_empty() {
        return Err("the command is empty".into());
    }
    if at.me.processes.contains_key(name) {
        return Err(format!(
            "a process named `{}` is already running: stop it first or choose another name",
            name.0
        ));
    }
    let process = Process {
        agent: at.id(),
        name: name.clone(),
        command: command.to_owned(),
    };
    Ok(Plan {
        events: vec![(Actor::Agent(at.id()), ProjectEvent::ProcessStarted(process))],
        process: Some(ProcessPlan::Start {
            name: name.clone(),
            command: command.to_owned(),
        }),
        ..Plan::default()
    })
}

pub(crate) fn stop(at: &At, name: &ProcessName) -> Result<Plan, Refusal> {
    if !at.me.processes.contains_key(name) {
        return Err(format!("no process named `{}` is running", name.0));
    }
    let stopped = ProjectEvent::ProcessStopped {
        agent: at.id(),
        name: name.clone(),
        end: ProcessEnd::Stopped,
    };
    Ok(Plan {
        events: vec![(Actor::Agent(at.id()), stopped)],
        process: Some(ProcessPlan::Stop { name: name.clone() }),
        ..Plan::default()
    })
}
