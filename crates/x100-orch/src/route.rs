//! Where the orchestrator asks the router, and what it records of the answers: the wait for a
//! driver, its deadline and who is told, the charge for a turn and the notices it causes, and
//! cool-offs. The rules themselves are all in `x100-router`.

use std::collections::BTreeSet;

use x100_core::agent::{AgentEvent, AgentState, Waiting};
use x100_core::driver::{Driver, Incompatible, PlannedReason, Purpose};
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessModel, ProviderFailure};
use x100_core::id::{AgentId, DriverId, JobId, MessageId, TaskId};
use x100_core::inbox::InboxItem;
use x100_core::job::{Job, JobEvent, JobState, ProjectJob, ProjectRoutine, Schedule};
use x100_core::ledger::{BudgetReached, CostBasis};
use x100_core::message::{Message, Route};
use x100_core::money::{Money, Usage};
use x100_core::party::Actor;
use x100_core::task::TaskFailure;
use x100_core::time::Clock;
use x100_router::{Choice, Decision, Ending, Need, Upkeep, Warm, World};

use crate::Shared;
use crate::ctx::{Events, ProjectCtx};
use crate::ids::new_id;
use crate::settle::deliver;
use crate::tools::{report_failed, who};

/// The router's decision, with the new driver's id and its room already taken.
pub(crate) enum Planned {
    Reuse {
        compact: bool,
    },
    Start {
        driver: DriverId,
        choice: Choice,
        ending: Option<Ending>,
    },
}

/// What the agent's next turn runs on, and the task it will be on. `failed_on` is the task the
/// agent's drivers last failed on and their harnesses and models, which a new driver for that
/// task avoids. Blocks.
pub(crate) fn decide(
    shared: &Shared,
    ctx: &ProjectCtx,
    agent: AgentId,
    warm: Option<Warm>,
    failed_on: Option<(TaskId, BTreeSet<HarnessModel>)>,
) -> (Result<Planned, Vec<Incompatible>>, Option<TaskId>) {
    let installed = shared.installed();
    let now = shared.clock.now();
    shared.fleet.allocate(|occupancy| {
        let (decision, task) = ctx.store.log.with_state(|p| {
            let task = p.current_task(agent).map(|t| t.task.id);
            let avoid = match &failed_on {
                Some((on, avoid)) if Some(*on) == task => avoid.clone(),
                _ => BTreeSet::new(),
            };
            let decision = shared.daemon.with_state(|d| {
                let world = World {
                    daemon: d,
                    project: p,
                    installed: &installed,
                    occupancy,
                    now,
                };
                let need = Need {
                    agent,
                    purpose: Purpose::Work,
                    warm,
                    avoid,
                };
                x100_router::choose(&world, &need).map(|decision| {
                    let slot = match &decision {
                        Decision::Start { choice, .. } => {
                            Some(x100_router::slot(d, choice.harness, choice.machine))
                        }
                        Decision::Reuse { .. } => None,
                    };
                    (decision, slot)
                })
            });
            (decision, task)
        });
        match decision {
            Ok((Decision::Reuse { compact, .. }, _)) => {
                ((Ok(Planned::Reuse { compact }), task), None)
            }
            Ok((Decision::Start { choice, ending }, slot)) => {
                let driver = new_id(&*shared.clock, DriverId::new);
                let slot = slot.expect("a start takes room");
                let planned = Planned::Start {
                    driver,
                    choice,
                    ending,
                };
                ((Ok(planned), task), Some((driver, slot)))
            }
            Err(why) => ((Err(why), task), None),
        }
    })
}

/// What becomes of a warm driver now. Blocks.
pub(crate) fn upkeep(shared: &Shared, ctx: &ProjectCtx, warm: &Warm) -> Upkeep {
    ctx.store
        .log
        .with_state(|p| upkeep_in(shared, p, warm, shared.clock.now()))
}

/// The same, over a state the caller holds, as a turn's end does inside its append.
pub(crate) fn upkeep_in(
    shared: &Shared,
    project: &ProjectState,
    warm: &Warm,
    now: x100_core::time::Timestamp,
) -> Upkeep {
    let installed = shared.installed();
    let occupancy = Default::default();
    shared.daemon.with_state(|d| {
        let world = World {
            daemon: d,
            project,
            installed: &installed,
            occupancy: &occupancy,
            now,
        };
        x100_router::upkeep(&world, warm)
    })
}

/// Why `driver`, whose agent carries on with its task, should hand the task to another driver,
/// over a state the caller holds.
pub(crate) fn hand_off_in(
    shared: &Shared,
    project: &ProjectState,
    driver: DriverId,
    now: x100_core::time::Timestamp,
) -> Option<PlannedReason> {
    let installed = shared.installed();
    let occupancy = Default::default();
    shared.daemon.with_state(|d| {
        let world = World {
            daemon: d,
            project,
            installed: &installed,
            occupancy: &occupancy,
            now,
        };
        x100_router::hand_off(&world, driver)
    })
}

/// The agent waits for a driver: why, its parent told why, and a deadline. Only an agent that was
/// working begins a wait; one already waiting keeps the reasons it was given.
pub(crate) fn no_driver(
    state: &ProjectState,
    clock: &dyn Clock,
    agent: AgentId,
    why: Vec<Incompatible>,
    deadline: std::time::Duration,
) -> Events {
    let Some(a) = state.agent(agent) else {
        return Vec::new();
    };
    if a.state != AgentState::Working {
        return Vec::new();
    }
    let mut out = vec![(
        Actor::Daemon,
        ProjectEvent::Agent {
            agent,
            event: AgentEvent::NoDriver(why.clone()),
        },
    )];
    let parent = state.project().and_then(|p| p.team.parent(agent));
    match parent {
        Some(parent) => {
            let item = InboxItem::NoDriver {
                agent,
                why: why.clone(),
            };
            let told = state
                .agent(parent)
                .is_some_and(|p| p.items.values().any(|i| *i == item));
            if !told {
                out.push(deliver(clock, parent, item));
            }
        }
        // The CEO answers to the builder, and could not be paid to say so itself.
        None => out.push(notice(
            clock,
            format!("The CEO is waiting for a driver: {}.", reasons(state, &why)),
        )),
    }
    if deadlines(state, agent).is_empty() {
        out.push((
            Actor::Daemon,
            ProjectEvent::JobScheduled(Job {
                id: new_id(clock, JobId::new),
                when: Schedule::Once {
                    at: clock.now() + deadline,
                },
                does: ProjectJob::Routine(ProjectRoutine::DriverWaitDeadline { agent }),
            }),
        ));
    }
    out
}

/// The agent has a driver again: its wait for one ends and its deadline is called off.
pub(crate) fn found(state: &ProjectState, agent: AgentId) -> Events {
    let mut out = Vec::new();
    if matches!(
        state.agent(agent).map(|a| &a.state),
        Some(AgentState::Waiting(Waiting::ForDriver(_)))
    ) {
        out.push((
            Actor::Daemon,
            ProjectEvent::Agent {
                agent,
                event: AgentEvent::DriverFound,
            },
        ));
    }
    for job in deadlines(state, agent) {
        out.push((
            Actor::Daemon,
            ProjectEvent::Job {
                job,
                event: JobEvent::Cancelled { by: Actor::Daemon },
            },
        ));
    }
    out
}

/// The deadline jobs of `agent`'s wait for a driver that can still fire.
fn deadlines(state: &ProjectState, agent: AgentId) -> Vec<JobId> {
    state
        .jobs()
        .filter(|j| matches!(j.state, JobState::Scheduled { .. }))
        .filter(|j| {
            matches!(&j.job.does, ProjectJob::Routine(ProjectRoutine::DriverWaitDeadline { agent: of })
                if *of == agent)
        })
        .map(|j| j.job.id)
        .collect()
}

/// The wait for a driver reached its deadline: every task the agent holds is reported failed to
/// its delegator, and the agent goes idle.
pub(crate) fn deadline_passed(state: &ProjectState, clock: &dyn Clock, agent: AgentId) -> Events {
    let Some(a) = state.agent(agent) else {
        return Vec::new();
    };
    let AgentState::Waiting(Waiting::ForDriver(why)) = &a.state else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for task in &a.open_tasks {
        let summary = format!("No driver could be found: {}.", reasons(state, why));
        out.extend(report_failed(
            state,
            clock,
            *task,
            TaskFailure::NoDriver(why.clone()),
            summary,
        ));
    }
    out.push((
        Actor::Daemon,
        ProjectEvent::Agent {
            agent,
            event: AgentEvent::WentIdle,
        },
    ));
    out
}

/// The charge for a turn, and for each budget it takes to its limit, the record and the notice:
/// to the CEO's inbox for a lead, in the chat for the project, since a CEO turn to say so could
/// not be paid for.
pub(crate) fn charged(
    state: &ProjectState,
    clock: &dyn Clock,
    driver: &Driver,
    usage: Usage,
    cost: Money,
    basis: CostBasis,
) -> Events {
    let Some(billed) = x100_router::bill(state, driver, usage, cost, basis) else {
        return Vec::new();
    };
    let mut out = vec![(Actor::Daemon, ProjectEvent::Charged(billed.charge))];
    for reached in billed.reached {
        out.push((Actor::Daemon, ProjectEvent::BudgetReached(reached)));
        match reached {
            BudgetReached::Lead(lead) => {
                if let Some(ceo) = state.project().map(|p| p.ceo) {
                    out.push(deliver(clock, ceo, InboxItem::BudgetReached { lead }));
                }
            }
            BudgetReached::Project => {
                let total = state.project().map_or(Money::ZERO, |p| p.settings.budget);
                out.push(notice(
                    clock,
                    format!(
                        "The project has reached its budget of {}. Nothing new starts until \
                         the budget is raised.",
                        dollars(total)
                    ),
                ));
            }
            BudgetReached::Lab => {}
        }
    }
    out
}

/// A harness and model failed in a way that looks like the provider's: they cool off for every
/// project, and the daemon's job clock is told of the job that ends it. Blocks.
pub(crate) fn cool_off(shared: &Shared, of: HarnessModel, why: ProviderFailure) {
    let ends = new_id(&*shared.clock, JobId::new);
    let now = shared.clock.now();
    let events = shared
        .daemon
        .with_state(|d| x100_router::cool_off(d, of, why, now, ends));
    match shared.daemon.append_all(events) {
        Ok(_) => shared.daemon_jobs.wake(),
        Err(e) => eprintln!("x100: a cool-off could not be recorded: {e}"),
    }
}

fn notice(clock: &dyn Clock, body: String) -> (Actor, ProjectEvent) {
    (
        Actor::Daemon,
        ProjectEvent::MessageSent(Message {
            id: new_id(clock, MessageId::new),
            route: Route::DaemonToBuilder,
            body,
            attachments: Vec::new(),
            in_reply_to: None,
        }),
    )
}

/// Why no driver fits, in words for the builder.
fn reasons(state: &ProjectState, why: &[Incompatible]) -> String {
    let words: Vec<String> = why
        .iter()
        .map(|w| match w {
            Incompatible::NoEffortInRange => {
                "nothing offered runs at an effort inside its range that meets its task".into()
            }
            Incompatible::NothingAllowed => "what would fit is not allowed in this project".into(),
            Incompatible::CoolingOff(of) => format!(
                "{} on harness {} is cooling off after a provider failure",
                of.model.0, of.harness
            ),
            Incompatible::MachineFull { .. } => "the machine is at its cap of drivers".into(),
            Incompatible::LoginFull { login } => format!(
                "the login at {} has as many terminal drivers as the daemon allows",
                login.0
            ),
            Incompatible::Budget(BudgetReached::Project) => {
                "the project has reached its budget".into()
            }
            Incompatible::Budget(BudgetReached::Lead(lead)) => format!(
                "{} has reached its budget",
                who(state, x100_core::party::Party::Agent(*lead))
            ),
            Incompatible::Budget(BudgetReached::Lab) => "the lab has reached its cap".into(),
        })
        .collect();
    words.join("; ")
}

fn dollars(m: Money) -> String {
    format!(
        "${}.{:02}",
        m.micros() / 1_000_000,
        m.micros() % 1_000_000 / 10_000
    )
}
