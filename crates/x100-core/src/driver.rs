//! Drivers: one harness running one model at one effort level on one machine, for one agent.
//!
//! A driver's agent is fixed in [`Driver`] when it starts and no [`DriverEvent`] names an agent, so
//! a driver acts for one agent for its whole life by construction.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::effort::Effort;
use crate::harness::{HarnessModel, Login, ModelName, ProviderFailure};
use crate::id::{ActionId, AgentId, DriverId, HarnessId, InboxItemId, MachineId, TaskId, TrialId};
use crate::ledger::BudgetReached;
use crate::machine::{MachineKind, Rejected};
use crate::trial::Arm;

/// After this many driver failures in a row on one task, with no turn completed between them, the
/// task is reported failed to its delegator.
pub const FAILURES_BEFORE_GIVING_UP: u32 = 3;

/// A driver as it starts: everything about it that never changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Driver {
    pub id: DriverId,
    pub agent: AgentId,
    pub harness: HarnessId,
    pub model: ModelName,
    pub effort: Effort,
    pub machine: MachineId,
    pub purpose: Purpose,
    /// How an interface shows this driver. Fixed at start, since it follows from how the harness is run.
    pub view: View,
    pub why: StartReason,
}

/// What the builder opens to watch a driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// The harness's own program runs in a terminal of its own, recorded as a terminal log.
    Terminal,
    /// No terminal exists; the transcript rendered as text is all there is to show.
    Headless,
}

/// A lab driver runs one arm of a trial in a throwaway sandbox. It does not act for its agent: its
/// calls never reach the project, so "one driver acts for an agent" counts work drivers only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Work,
    Lab { trial: TrialId, arm: Arm, run: u8 },
}

/// Why a driver is starting. Every start rebuilds context from the start pack; this says what the
/// pack is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartReason {
    NewTask {
        task: TaskId,
    },
    /// Anything else in the inbox: a message, a report, a decision, a job firing, a wait ending.
    Wake,
    Handoff(Handoff),
}

/// The new driver's record of the one it replaced. This is the handoff event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Handoff {
    pub replaces: DriverId,
    pub cause: HandoffCause,
    pub in_flight: InFlight,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffCause {
    /// The old driver cannot help.
    Takeover(TakeoverCause),
    /// The old driver is healthy but should not go on. It had a last turn to bring the progress
    /// note up to date.
    Planned(PlannedReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeoverCause {
    Failed(Failure),
    Stopped(StopReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannedReason {
    /// Its context passed the threshold mid-task.
    ContextFull,
    /// Its task's effort changed and it no longer fits.
    EffortChanged,
    /// The builder disallowed its harness or model.
    Disallowed,
    /// The agent's effort range was narrowed below it.
    RangeNarrowed,
}

/// What the old driver was doing when it stopped, so the new one neither repeats nor loses it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InFlight {
    Nothing,
    /// A daemon tool call, which the daemon finishes. Its result is in the start pack.
    ToolCall {
        action: ActionId,
    },
    /// A command in the sandbox, whose outcome is unknown.
    Command {
        command: String,
    },
}

/// Why a driver failed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    ProcessExited {
        code: Option<i32>,
    },
    StreamBroke,
    /// An error it reported and cannot retry. `provider` is set when it looks like the provider's
    /// fault, which makes the harness and model cool off.
    Error {
        detail: String,
        provider: Option<ProviderFailure>,
    },
    Stalled {
        silent_for: Duration,
    },
    /// The last turn of a planned handoff failed or ran out of time, so the handoff became a
    /// takeover.
    LastTurnFailed,
    /// The daemon restarted while it was mid-turn. Drivers do not outlive the daemon.
    DaemonRestarted,
}

impl Failure {
    /// Whether this failure is held against the driver's harness and model when choosing the next
    /// driver for the same task.
    pub fn counts_against_harness(&self) -> bool {
        !matches!(self, Failure::DaemonRestarted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetireReason {
    ContextFull,
    TooManyTasks,
    WarmTooLong,
    /// It has been alive longer than the daemon's setting allows.
    TooOld,
    /// Its task was accepted and its harness cannot compact.
    CannotCompact,
    AgentRetired,
    /// It is healthy, but between turns it no longer fits its agent's work. Retired, not handed
    /// off: between turns there is nothing left for a last turn to write.
    Unfit(Unfit),
    /// It worked in a scratch directory because the project had no code yet. The project is set
    /// up now, and a driver cannot move into the code directory, so a new one starts there.
    ProjectSetUp,
}

/// Why a warm driver no longer fits its agent's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unfit {
    /// The builder disallowed its harness or model.
    Disallowed,
    /// Its harness and model are cooling off after a provider failure.
    CoolingOff,
    /// The agent's effort range was narrowed and no longer holds it.
    RangeNarrowed,
    /// Its agent's task needs more effort than it runs at.
    EffortChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    ReplacedByBuilder,
    /// It was warm when the daemon restarted.
    DaemonRestarted,
    LabRunEnded,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Retired(RetireReason),
    HandedOff(PlannedReason),
    Stopped(StopReason),
}

/// How full a driver's context is, as its harness reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextFill {
    pub used: u64,
    pub window: u64,
}

/// Why no compatible driver could be found for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Incompatible {
    NoEffortInRange,
    NothingAllowed,
    CoolingOff(HarnessModel),
    MachineFull {
        machine: MachineId,
    },
    /// The login its terminal drivers run on has as many live as the daemon allows.
    LoginFull {
        login: Login,
    },
    Budget(BudgetReached),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverState {
    Starting,
    /// In a turn.
    Running,
    /// Between turns. Still holds a process, and spends nothing.
    Warm,
    Compacting,
    /// Running its last turn before a planned handoff.
    HandingOff(PlannedReason),
    Ended(EndReason),
    Failed(Failure),
}

impl DriverState {
    pub const INITIAL: DriverState = DriverState::Starting;

    /// Live is any state that is not final. A machine's cap counts live drivers.
    pub fn is_live(&self) -> bool {
        !matches!(self, DriverState::Ended(_) | DriverState::Failed(_))
    }

    pub fn name(&self) -> &'static str {
        match self {
            DriverState::Starting => "Starting",
            DriverState::Running => "Running",
            DriverState::Warm => "Warm",
            DriverState::Compacting => "Compacting",
            DriverState::HandingOff(_) => "HandingOff",
            DriverState::Ended(_) => "Ended",
            DriverState::Failed(_) => "Failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverEvent {
    /// A turn begins with these inbox items. They are handled only if this turn ends.
    TurnStarted {
        given: Vec<InboxItemId>,
    },
    TurnEnded,
    ContextReported(ContextFill),
    CompactionStarted,
    Compacted,
    /// A planned handoff begins, and with it the driver's last turn.
    HandoffStarted(PlannedReason),
    Retired(RetireReason),
    Stopped(StopReason),
    Failed(Failure),
}

impl DriverEvent {
    pub fn name(&self) -> &'static str {
        match self {
            DriverEvent::TurnStarted { .. } => "TurnStarted",
            DriverEvent::TurnEnded => "TurnEnded",
            DriverEvent::ContextReported(_) => "ContextReported",
            DriverEvent::CompactionStarted => "CompactionStarted",
            DriverEvent::Compacted => "Compacted",
            DriverEvent::HandoffStarted(_) => "HandoffStarted",
            DriverEvent::Retired(_) => "Retired",
            DriverEvent::Stopped(_) => "Stopped",
            DriverEvent::Failed(_) => "Failed",
        }
    }
}

/// The driver machine. Starting → Running → Warm → Running again, or Warm → Compacting → Warm. Warm
/// → Handing off → Ended for a planned handoff. Warm → Ended when retired. Any live state → Ended
/// when stopped, or → Failed.
pub fn next(state: &DriverState, event: &DriverEvent) -> Result<DriverState, Rejected> {
    use DriverEvent as E;
    use DriverState as S;
    let next = match (state, event) {
        (S::Ended(_) | S::Failed(_), _) => None,
        (S::Starting | S::Warm, E::TurnStarted { .. }) => Some(S::Running),
        (S::Running, E::TurnEnded) => Some(S::Warm),
        (S::HandingOff(reason), E::TurnEnded) => Some(S::Ended(EndReason::HandedOff(*reason))),
        (live, E::ContextReported(_)) => Some(live.clone()),
        (S::Warm, E::CompactionStarted) => Some(S::Compacting),
        (S::Compacting, E::Compacted) => Some(S::Warm),
        (S::Warm, E::HandoffStarted(reason)) => Some(S::HandingOff(*reason)),
        (S::Warm, E::Retired(why)) => Some(S::Ended(EndReason::Retired(*why))),
        (_, E::Stopped(why)) => Some(S::Ended(EndReason::Stopped(*why))),
        (_, E::Failed(failure)) => Some(S::Failed(failure.clone())),
        _ => None,
    };
    next.ok_or(Rejected {
        machine: MachineKind::Driver,
        state: state.name(),
        event: event.name(),
    })
}
