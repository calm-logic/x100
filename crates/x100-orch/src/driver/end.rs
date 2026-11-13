//! How a driver ends, and what it leaves the next one, a planned handoff's last turn included.
//!
//! An end is recorded in this order, whatever caused it. The desk is closed to the driver first,
//! and every call it made before is carried out, so a call in flight takes effect once and is in
//! the log before anything is built from it. Then the end is recorded, which fences the driver
//! off: the log no longer shows it acting for its agent. Then its process is stopped, if it is not
//! already gone, and the worktree is checkpointed as it stands. What it was doing when it stopped
//! goes to the loop with how it ended, for the next driver's record and start pack.
//!
//! A planned handoff's last turn has no inbox items and a time limit on the agent's clock. If it
//! fails or runs out of time, the handoff becomes a takeover.

use std::sync::Arc;

use x100_core::driver::{DriverEvent, Failure, InFlight, PlannedReason, StopReason};
use x100_core::event::ProjectEvent;
use x100_core::git::CommitId;
use x100_core::harness::HarnessModel;
use x100_core::id::AgentId;
use x100_core::job::JobEvent;
use x100_core::party::Actor;
use x100_core::transcript::{Item, TurnEnd as Ended};
use x100_drivers::DriverControl;

use super::{Actor_, How, Turn};
use crate::ctx::{Events, ProjectCtx};
use crate::pack;
use crate::redact;
use crate::route;
use crate::store::blocking;
use crate::tools::Tool;
use crate::turns;

/// How a driver's life ended, as its task tells the project's loop.
#[derive(Debug, Clone)]
pub(crate) enum Exit {
    /// The router retired it, or the daemon is stopping: nothing is handed on.
    Quiet,
    /// It failed, which counts toward the three in a row.
    Failed {
        failure: Failure,
        in_flight: InFlight,
    },
    /// The builder replaced it.
    Replaced { in_flight: InFlight },
    /// It handed its agent's task off after its last turn.
    HandedOff(PlannedReason),
}

impl Actor_ {
    /// The driver failed: the turn it was in is lost and its items wait for the next driver. A
    /// failure that looks like the provider's cools its harness and model off for every project.
    /// A failure in a planned handoff's last turn makes the handoff a takeover.
    pub(super) async fn fail(&mut self, failure: Failure) -> Exit {
        let failure = if self.handing_off.is_some() {
            Failure::LastTurnFailed
        } else {
            failure
        };
        // A failure carries what the program last wrote, which is the driver's own output.
        let failure = redact::value(&self.shared.redactor, &failure).unwrap_or(Failure::Error {
            detail: "the driver failed, and how could not be cleared of credential values".into(),
            provider: None,
        });
        if let (
            Failure::Error {
                provider: Some(why),
                ..
            },
            Some(r),
        ) = (&failure, &self.record)
        {
            let of = HarnessModel {
                harness: r.harness,
                model: r.model.clone(),
            };
            let (shared, why) = (Arc::clone(&self.shared), why.clone());
            blocking(move || route::cool_off(&shared, of, why)).await;
        }
        if self.turn.take().is_some() {
            let detail = format!("{failure:?}");
            self.transcript(Item::TurnEnded {
                how: Ended::Failed { detail },
            })
            .await;
        }
        let in_flight = self.end(vec![DriverEvent::Failed(failure.clone())]).await;
        self.checkpoint_now().await;
        Exit::Failed { failure, in_flight }
    }

    /// Stops the driver for the builder or for the daemon's stopping.
    pub(super) async fn stopped(&mut self, how: How, control: &DriverControl) -> Exit {
        let mid_turn = self.turn.take().is_some();
        if mid_turn {
            self.transcript(Item::TurnEnded {
                how: Ended::Interrupted,
            })
            .await;
        }
        let event = match how {
            How::Builder => DriverEvent::Stopped(StopReason::ReplacedByBuilder),
            How::Shutdown if mid_turn => DriverEvent::Failed(Failure::DaemonRestarted),
            How::Shutdown => DriverEvent::Stopped(StopReason::DaemonRestarted),
        };
        let in_flight = self.end(vec![event]).await;
        control.stop().await;
        self.checkpoint_now().await;
        match how {
            How::Builder => Exit::Replaced { in_flight },
            How::Shutdown => Exit::Quiet,
        }
    }

    /// Gives a driver that is handing off its last turn, which has no inbox items: those wait for
    /// the next driver.
    pub(super) async fn last_turn(&mut self, control: &DriverControl, reason: PlannedReason) {
        let agent = self.agent;
        let task = self
            .read(move |s| s.current_task(agent).map(|t| t.task.id))
            .await;
        self.handing_off = Some((reason, self.ctx.clock.now()));
        self.turn = Some(Turn { given: Vec::new() });
        self.transcript(Item::TurnStarted).await;
        let _ = control.turn(pack::last_turn(reason, task)).await;
    }

    pub(super) fn last_turn_overdue(&self) -> bool {
        let limit = (self.shared.settings)().last_turn_limit;
        self.handing_off
            .is_some_and(|(_, began)| self.ctx.clock.now().since(began) > limit)
    }

    /// The last turn of a planned handoff completed: the work is checkpointed and the driver
    /// ends, handed off. Its caller stops the process.
    pub(super) async fn last_turn_ended(&mut self, reason: PlannedReason) -> Result<Exit, String> {
        self.turn = None;
        self.transcript(Item::TurnEnded {
            how: Ended::Completed,
        })
        .await;
        self.shared.desk.close(self.driver).await;
        let (ctx, agent, driver) = (Arc::clone(&self.ctx), self.agent, self.driver);
        blocking(move || {
            let commit = checkpoint(&ctx, agent);
            ctx.commit(|s, _| (turns::last_turn_ended(s, agent, driver, commit), ()))
        })
        .await
        .map_err(|e| format!("the log refused the end of a last turn: {e}"))?;
        Ok(Exit::HandedOff(reason))
    }

    /// Closes the desk to the driver, then records its end and calls off its warm period, which
    /// has no driver left to end. Returns what it was doing when it ended.
    pub(super) async fn end(&mut self, ending: Vec<DriverEvent>) -> InFlight {
        let last = self.shared.desk.close(self.driver).await;
        let in_flight = in_flight(&self.open, last);
        self.open.clear();
        let (ctx, driver) = (Arc::clone(&self.ctx), self.driver);
        let ended = blocking(move || {
            ctx.commit(|s, _| {
                let mut events: Events = ending
                    .into_iter()
                    .map(|event| (Actor::Daemon, ProjectEvent::Driver { driver, event }))
                    .collect();
                if let Some(job) = turns::warm_period_job(s, driver) {
                    let cancel = JobEvent::Cancelled { by: Actor::Daemon };
                    events.push((Actor::Daemon, ProjectEvent::Job { job, event: cancel }));
                }
                (events, ())
            })
        })
        .await;
        if let Err(e) = ended {
            eprintln!(
                "x100: the end of driver {} is not in the log: {e}",
                self.driver
            );
        }
        in_flight
    }

    /// The worktree as the driver left it, once it can change it no more.
    async fn checkpoint_now(&self) {
        let (ctx, agent) = (Arc::clone(&self.ctx), self.agent);
        let done = blocking(move || {
            let Some(commit) = checkpoint(&ctx, agent) else {
                return Ok(());
            };
            let event = ProjectEvent::Checkpointed { agent, commit };
            ctx.commit(|_, _| (vec![(Actor::Daemon, event)], ()))
        })
        .await;
        if let Err(e) = done {
            eprintln!("x100: the checkpoint of {agent} is not in the log: {e}");
        }
    }
}

/// Commits the agent's worktree to its hidden checkpoint ref, if the project has code and the
/// worktree is there. Blocks.
pub(crate) fn checkpoint(ctx: &ProjectCtx, agent: AgentId) -> Option<CommitId> {
    let git = ctx.git()?;
    let dir = git.worktree_path(agent);
    if !dir.exists() {
        return None;
    }
    git.checkpoint(&dir, &agent.to_string())
        .inspect_err(|e| eprintln!("x100: could not checkpoint {agent}: {e}"))
        .ok()
}

/// What the driver was doing when it ended: the last tool call it began and had no result of. A
/// daemon tool call is named by its action if it reached the log, which the desk's close made
/// sure of; one that never reached the daemon took no effect, and nothing is in flight. Any other
/// tool call is a command whose outcome is unknown.
fn in_flight(open: &[super::Open], last: Option<(x100_core::id::ActionId, Tool)>) -> InFlight {
    let Some(call) = open.last() else {
        return InFlight::Nothing;
    };
    match daemon_tool(&call.name) {
        Some(tool) => match last {
            Some((action, t)) if t == tool => InFlight::ToolCall { action },
            _ => InFlight::Nothing,
        },
        None => InFlight::Command {
            command: command_of(&call.input),
        },
    }
}

/// The daemon tool a harness's tool call names: the fake harness names it bare, and both real
/// harnesses as `mcp__<server>__<tool>`.
fn daemon_tool(name: &str) -> Option<Tool> {
    Tool::named(name.rsplit("__").next().unwrap_or(name))
}

/// The command a tool call ran, as its harness reported it.
fn command_of(input: &serde_json::Value) -> String {
    const LONGEST: usize = 400;
    let text = match input.get("command") {
        Some(serde_json::Value::String(c)) => c.clone(),
        Some(other) => other.to_string(),
        None => input.to_string(),
    };
    text.chars().take(LONGEST).collect()
}
