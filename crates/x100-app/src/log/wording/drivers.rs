//! Wording for drivers: why none fits, handoffs, failures and what a driver's events say.

use x100_core::driver::{
    DriverEvent, Failure, Handoff, HandoffCause, InFlight, Incompatible, PlannedReason,
    RetireReason, StopReason, TakeoverCause,
};
use x100_core::harness::{OneOffCall, OneOffPurpose};
use x100_core::id::DriverId;
use x100_core::ledger::BudgetReached;

use super::{Names, duration, snippet};
use crate::words;

pub(super) fn incompatible(n: &Names, why: &Incompatible) -> String {
    match why {
        Incompatible::NoEffortInRange => "no allowed driver offers an effort in its range".into(),
        Incompatible::NothingAllowed => "no harness and model is allowed".into(),
        Incompatible::CoolingOff(what) => format!(
            "{} \u{b7} {} is cooling off",
            words::harness(n.daemon, what.harness),
            what.model.0
        ),
        Incompatible::MachineFull { .. } => "the machine is full".into(),
        Incompatible::Budget(b) => match b {
            BudgetReached::Project => "the project is at its budget".into(),
            BudgetReached::Lead(lead) => format!("{} is at its budget", n.agent(*lead)),
            BudgetReached::Lab => "the lab is at its cap".into(),
        },
    }
}

pub(super) fn handoff(n: &Names, h: &Handoff) -> String {
    let from = n.driver(h.replaces);
    let cause = match &h.cause {
        HandoffCause::Takeover(TakeoverCause::Failed(f)) => format!("it failed: {}", failure(f)),
        HandoffCause::Takeover(TakeoverCause::Stopped(s)) => stop_reason(*s).to_owned(),
        HandoffCause::Planned(p) => planned(*p).to_owned(),
    };
    let left = match &h.in_flight {
        InFlight::Nothing => String::new(),
        InFlight::ToolCall { .. } => ", carrying on from a tool call in flight".into(),
        InFlight::Command { command } => format!(", carrying on from the command {command}"),
    };
    format!("taking over from {from} because {cause}{left}")
}

fn failure(f: &Failure) -> String {
    match f {
        Failure::ProcessExited { code: Some(c) } => format!("the process exited with code {c}"),
        Failure::ProcessExited { code: None } => "the process was ended by a signal".into(),
        Failure::StreamBroke => "its stream broke".into(),
        Failure::Error { detail, .. } => snippet(detail),
        Failure::Stalled { silent_for } => format!("it was silent for {}", duration(*silent_for)),
        Failure::LastTurnFailed => "its last turn failed".into(),
        Failure::DaemonRestarted => "the daemon restarted".into(),
    }
}

fn planned(p: PlannedReason) -> &'static str {
    match p {
        PlannedReason::ContextFull => "its context was full",
        PlannedReason::EffortChanged => "the effort the task needs changed",
        PlannedReason::Disallowed => "the builder stopped allowing it",
        PlannedReason::RangeNarrowed => "the effort range was narrowed",
    }
}

fn stop_reason(s: StopReason) -> &'static str {
    match s {
        StopReason::ReplacedByBuilder => "the builder replaced it",
        StopReason::DaemonRestarted => "the daemon restarted",
        StopReason::LabRunEnded => "the lab run ended",
    }
}

pub(super) fn driver_event(n: &Names, driver: DriverId, event: &DriverEvent) -> String {
    let who = n.driver_agent(driver);
    match event {
        DriverEvent::TurnStarted { .. } => format!("{who} started a turn"),
        DriverEvent::TurnEnded => format!("{who} finished a turn"),
        DriverEvent::ContextReported(fill) => {
            format!("{who}'s context is {} full", words::context(Some(*fill)))
        }
        DriverEvent::CompactionStarted => format!("Started compacting {who}'s context"),
        DriverEvent::Compacted => format!("Compacted {who}'s context"),
        DriverEvent::HandoffStarted(reason) => format!(
            "Handing {who}'s work to a new driver because {}",
            planned(*reason)
        ),
        DriverEvent::Retired(reason) => format!(
            "Retired {who}'s driver: {}",
            match reason {
                RetireReason::ContextFull => "its context was full",
                RetireReason::TooManyTasks => "it had done many tasks",
                RetireReason::WarmTooLong => "it was warm too long",
                RetireReason::TooOld => "it was too old",
                RetireReason::CannotCompact => "it could not compact",
                RetireReason::AgentRetired => "the agent was retired",
            }
        ),
        DriverEvent::Stopped(reason) => {
            format!("Stopped {who}'s driver: {}", stop_reason(*reason))
        }
        DriverEvent::Failed(f) => format!("{who}'s driver failed: {}", failure(f)),
    }
}

pub(super) fn one_off(n: &Names, c: &OneOffCall) -> String {
    let by = format!(
        "{} \u{b7} {}",
        words::harness(n.daemon, c.harness),
        c.model.0
    );
    match c.purpose {
        OneOffPurpose::Classify { action } => {
            format!("Asked {by} to classify {}", n.action(action))
        }
        OneOffPurpose::Judge { .. } => format!("Asked {by} to judge a trial"),
    }
}
