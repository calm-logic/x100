//! Why a driver is starting, in the words its pack ends with, and the input of a planned
//! handoff's last turn. A takeover says why the last driver stopped and what it was doing: a
//! daemon tool call, which the daemon finished and whose result is given here, or a command,
//! whose outcome nobody knows.

use std::fmt::Write as _;

use x100_core::driver::{
    DriverState, EndReason, Failure, Handoff, HandoffCause, InFlight, PlannedReason, StartReason,
    StopReason, TakeoverCause,
};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, TaskId};

use super::history::History;
use crate::tools::Tool;

pub(super) fn reason(
    state: &ProjectState,
    agent: AgentId,
    why: &StartReason,
    history: &History,
) -> String {
    let h = match why {
        StartReason::NewTask { task } => {
            return format!("A new task, {task}, reached you.{}", previous(state, agent));
        }
        StartReason::Wake => {
            return format!("Something reached your inbox.{}", previous(state, agent));
        }
        StartReason::Handoff(h) => h,
    };
    let mut text = format!(
        "You take over the work of your previous driver, {}, which {}.",
        h.replaces,
        cause(&h.cause)
    );
    text.push(' ');
    text.push_str(&in_flight(h, history));
    text.push_str(
        " Your sandbox, your files and your task are as it left them. Carry on from the progress \
         note and the records above, and do again nothing the calls above say took effect.",
    );
    text
}

/// The input of a planned handoff's last turn. Labelled, as every turn's input is, so a model
/// finds the task's id.
pub(crate) fn last_turn(reason: PlannedReason, task: Option<TaskId>) -> String {
    let mut text = format!(
        "HAND OFF: this is the last turn of this driver, since {}. Another driver carries the task \
         on from your records. Bring the task's progress note up to date with update_task: what is \
         done, what is next, and what was decided. Keep what you should with remember. Start no \
         new work, and end the turn once the note is written.",
        planned(reason)
    );
    if let Some(task) = task {
        let _ = write!(text, "\nTask: {task}");
    }
    text
}

fn cause(cause: &HandoffCause) -> String {
    match cause {
        HandoffCause::Takeover(TakeoverCause::Failed(f)) => format!("failed: {}", failure(f)),
        HandoffCause::Takeover(TakeoverCause::Stopped(s)) => match s {
            StopReason::ReplacedByBuilder => "the builder replaced".into(),
            StopReason::DaemonRestarted => "stopped when the daemon restarted".into(),
            StopReason::LabRunEnded => "ended with its lab run".into(),
        },
        HandoffCause::Planned(p) => format!(
            "handed the task on after bringing its progress note up to date, since {}",
            planned(*p)
        ),
    }
}

fn planned(reason: PlannedReason) -> &'static str {
    match reason {
        PlannedReason::ContextFull => "its context passed the daemon's share of the window",
        PlannedReason::EffortChanged => "the task's effort changed and it no longer fits",
        PlannedReason::Disallowed => "the builder disallowed its harness or model",
        PlannedReason::RangeNarrowed => "the agent's effort range was narrowed below it",
    }
}

fn failure(f: &Failure) -> String {
    match f {
        Failure::ProcessExited { code: Some(code) } => {
            format!("its process exited with code {code}")
        }
        Failure::ProcessExited { code: None } => "its process was killed".into(),
        Failure::StreamBroke => "its output stream broke".into(),
        Failure::Error { detail, .. } => format!("it reported an error, {detail}"),
        Failure::Stalled { silent_for } => format!(
            "it was silent for {} seconds, past the stall timeout",
            silent_for.as_secs()
        ),
        Failure::LastTurnFailed => {
            "its last turn before a planned handoff failed or ran out of time".into()
        }
        Failure::DaemonRestarted => "the daemon restarted while it was working".into(),
    }
}

fn in_flight(h: &Handoff, history: &History) -> String {
    match &h.in_flight {
        InFlight::Nothing => "Nothing it started was in flight when it stopped.".into(),
        InFlight::ToolCall { action } => {
            let Some(call) = history.calls.iter().find(|c| c.action == *action) else {
                return format!(
                    "It was making a daemon tool call when it stopped, action {action}, which the \
                     daemon finished; it changed nothing that is recorded."
                );
            };
            let result = match &call.result {
                Some(Ok(r)) => format!("it took effect, and its result was: {r}"),
                Some(Err(e)) => format!("it was refused: {e}"),
                None => "it is still being carried out".into(),
            };
            format!(
                "It was making a daemon tool call when it stopped, {} (action {action}), which the \
                 daemon finished: {result}. Do not make it again.",
                Tool::of(&call.call).name()
            )
        }
        InFlight::Command { command } => format!(
            "It was running the command `{command}` when it stopped. Its outcome is unknown: \
             check what it did before relying on it."
        ),
    }
}

/// How the agent's previous driver ended, for a start that is not a handoff.
fn previous(state: &ProjectState, agent: AgentId) -> String {
    // The driver being started is in the record already, and live.
    let previous = state.agent(agent).and_then(|a| {
        a.drivers
            .iter()
            .rev()
            .filter_map(|d| state.driver(*d))
            .find(|d| !d.state.is_live())
    });
    let Some(d) = previous else {
        return String::new();
    };
    let ended = match &d.state {
        DriverState::Failed(f) => format!("failed ({}); what it was doing is lost", failure(f)),
        DriverState::Ended(EndReason::Retired(r)) => format!("retired ({r:?})"),
        DriverState::Ended(EndReason::Stopped(s)) => format!("was stopped ({s:?})"),
        DriverState::Ended(other) => format!("ended ({other:?})"),
        _ => "ended".to_owned(),
    };
    format!(
        " Your previous driver, {}, {ended}. Carry on from the progress note and the records \
         above.",
        d.driver.id
    )
}
