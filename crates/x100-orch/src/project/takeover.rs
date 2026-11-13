//! What the loop keeps between a driver that ended mid-task and its agent's next: the handoff the
//! next starts in, the harnesses and models that failed on the task, and the count of failures in
//! a row that gives the task up.

use std::collections::BTreeSet;
use std::sync::Arc;

use x100_core::agent::AgentState;
use x100_core::driver::{
    DriverState, EndReason, FAILURES_BEFORE_GIVING_UP, Failure, Handoff, HandoffCause, InFlight,
    StopReason, TakeoverCause,
};
use x100_core::event::ProjectEvent;
use x100_core::harness::HarnessModel;
use x100_core::id::{AgentId, DriverId};
use x100_core::party::Actor;
use x100_core::task::TaskFailure;

use super::Loop;
use crate::driver::{self, Exit};
use crate::store::blocking;
use crate::tools;

impl Loop {
    /// A driver ended. If its agent is mid-task, its next driver takes the task over: the cause,
    /// and what was in flight, go in that driver's record. A failure is held against its harness
    /// and model for the task, and counts toward giving the task up.
    pub(super) async fn exited(&mut self, agent: AgentId, driver: DriverId, exit: Exit) {
        let (cause, in_flight) = match &exit {
            Exit::Quiet => return,
            Exit::Failed { failure, in_flight } => (
                HandoffCause::Takeover(TakeoverCause::Failed(failure.clone())),
                in_flight.clone(),
            ),
            Exit::Replaced { in_flight } => (
                HandoffCause::Takeover(TakeoverCause::Stopped(StopReason::ReplacedByBuilder)),
                in_flight.clone(),
            ),
            Exit::HandedOff(reason) => (HandoffCause::Planned(*reason), InFlight::Nothing),
        };
        let ctx = Arc::clone(&self.ctx);
        let (task, of) = blocking(move || {
            ctx.store.log.with_state(|s| {
                let of = s.driver(driver).map(|d| HarnessModel {
                    harness: d.driver.harness,
                    model: d.driver.model.clone(),
                });
                (s.current_task(agent).map(|t| t.task.id), of)
            })
        })
        .await;
        let Some(task) = task else {
            return;
        };
        self.handoffs.insert(
            agent,
            Handoff {
                replaces: driver,
                cause,
                in_flight,
            },
        );
        if let Exit::Failed { failure, .. } = exit {
            if let Some(of) = of.filter(|_| failure.counts_against_harness()) {
                let entry = self
                    .failed_on
                    .entry(agent)
                    .or_insert((task, BTreeSet::new()));
                if entry.0 != task {
                    *entry = (task, BTreeSet::new());
                }
                entry.1.insert(of);
            }
            self.failed(agent, failure).await;
        }
    }

    /// After a restart: every agent that was working when the daemon stopped is taken over on a
    /// new driver, which is told its last one was ended by the restart. The worktree is
    /// checkpointed as the driver left it.
    pub(super) async fn after_restart(&mut self) {
        let ctx = Arc::clone(&self.ctx);
        let restarted: Vec<(AgentId, Handoff)> = blocking(move || {
            ctx.store.log.with_state(|s| {
                s.agents()
                    .filter(|a| a.state == AgentState::Working && s.current_task(a.id).is_some())
                    .filter_map(|a| {
                        let last = a.drivers.iter().rev().find_map(|d| {
                            s.driver(*d)
                                .filter(|r| r.driver.purpose == x100_core::driver::Purpose::Work)
                        })?;
                        let cause = match &last.state {
                            DriverState::Failed(Failure::DaemonRestarted) => {
                                TakeoverCause::Failed(Failure::DaemonRestarted)
                            }
                            DriverState::Ended(EndReason::Stopped(StopReason::DaemonRestarted)) => {
                                TakeoverCause::Stopped(StopReason::DaemonRestarted)
                            }
                            _ => return None,
                        };
                        let handoff = Handoff {
                            replaces: last.driver.id,
                            cause: HandoffCause::Takeover(cause),
                            in_flight: InFlight::Nothing,
                        };
                        Some((a.id, handoff))
                    })
                    .collect()
            })
        })
        .await;
        for (agent, handoff) in restarted {
            let ctx = Arc::clone(&self.ctx);
            let checkpointed = blocking(move || {
                let Some(commit) = driver::checkpoint(&ctx, agent) else {
                    return Ok(());
                };
                let event = ProjectEvent::Checkpointed { agent, commit };
                ctx.commit(|_, _| (vec![(Actor::Daemon, event)], ()))
            })
            .await;
            if let Err(e) = checkpointed {
                eprintln!("x100: the checkpoint of {agent} is not in the log: {e}");
            }
            self.handoffs.insert(agent, handoff);
        }
    }

    /// Three failures in a row with no turn completed between: the agent stops trying, and its
    /// task is reported failed to its delegator, with each failure.
    async fn failed(&mut self, agent: AgentId, failure: Failure) {
        let failures = self.failures.entry(agent).or_default();
        failures.push(failure);
        if (failures.len() as u32) < FAILURES_BEFORE_GIVING_UP {
            return;
        }
        let failures = self.failures.remove(&agent).unwrap_or_default();
        self.gave_up.insert(agent);
        self.handoffs.remove(&agent);
        self.failed_on.remove(&agent);
        self.commit(move |s, clock| {
            let Some(task) = s.current_task(agent).map(|t| t.task.id) else {
                return Vec::new();
            };
            let summary = format!("Its drivers failed {} times in a row.", failures.len());
            tools::report_failed(
                s,
                clock,
                task,
                TaskFailure::DriverFailures(failures),
                summary,
            )
        })
        .await;
    }
}
