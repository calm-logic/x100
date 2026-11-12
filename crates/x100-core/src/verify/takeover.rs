//! One agent, the task delegated to it, its inbox, and the drivers it has one after another, under
//! driver failures, a builder replacing a driver, a planned handoff, and a time when no compatible
//! driver can be found. It checks:
//!
//! - "Every delegated task ends in a report to its delegator, even when the assignee's drivers
//!   fail."
//! - "At most one driver acts for an agent at any time."
//! - "Nothing delivered to an agent is lost: every inbox item is handled by a turn that completed."
//! - The driver half of "no agent waits forever": waiting for a driver ends.
//!
//! The real agent, driver, task and job machines and the real inbox decide every step. What the
//! model adds is the orchestrator's policy from the architecture, which M6 and M9 build: fence the
//! old driver before starting a new one, give the task up after three failures in a row, nudge an
//! agent that neither reports nor waits, and report the task failed when the wait for a driver
//! reaches its deadline, after which the agent goes idle and keeps any unhandled items for its
//! next wake.

use std::collections::{BTreeMap, BTreeSet};

use stateright::{Model, Property};

use super::{is_terminal, prove, refute};
use crate::Rejected;
use crate::agent::{self, AgentEvent, AgentState, Waiting};
use crate::driver::{
    self, DriverEvent, DriverState, FAILURES_BEFORE_GIVING_UP, Failure, Incompatible,
    PlannedReason, RetireReason, StopReason,
};
use crate::effort::Effort;
use crate::id::{AgentId, DriverId, InboxItemId, TaskId};
use crate::inbox::{Inbox, InboxError, InboxItem};
use crate::job::{self, JobEvent, JobState, Schedule};
use crate::party::{Actor, Party};
use crate::task::{self, Assignment, Outcome, Report, TaskEvent, TaskFailure, TaskState, Verdict};
use crate::time::Timestamp;

type DriverNext = fn(&DriverState, &DriverEvent) -> Result<DriverState, Rejected>;
type TaskNext = fn(&TaskState, &TaskEvent) -> Result<TaskState, Rejected>;
type Give = fn(&mut Inbox, &[InboxItemId], DriverId) -> Result<(), InboxError>;

/// The real code the model runs. Broken variants replace one of these.
#[derive(Clone, Copy)]
struct Machines {
    driver: DriverNext,
    task: TaskNext,
    give: Give,
}

const REAL: Machines = Machines {
    driver: driver::next,
    task: task::next,
    give: Inbox::give,
};

/// Driver failures in one run of the model: enough for three in a row.
const MAX_FAILURES: u8 = 3;
const NUDGE_LIMIT: u8 = 2;
const TASK: TaskId = TaskId::from_u128(1);
const DELEGATOR: AgentId = AgentId::from_u128(99);

struct Takeover {
    m: Machines,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct State {
    agent: AgentState,
    task: TaskState,
    drivers: Vec<DriverState>,
    /// The driver the daemon takes to be the agent's current one. Calls from any other are refused.
    current: Option<usize>,
    inbox: Inbox,
    delivered: u8,
    /// What the model saw handled by a turn that completed.
    handled: BTreeSet<InboxItemId>,
    /// What each driver's running turn was given, as the model saw it.
    in_turn: BTreeMap<usize, Vec<InboxItemId>>,
    failures: u8,
    in_a_row: u8,
    /// Three failures in a row: the agent stops trying the task.
    gave_up: bool,
    nudges: u8,
    driver_wait: Option<JobState>,
    /// The delegator has the report for the task as it now stands.
    reported: bool,
    message_sent: bool,
    no_driver_once: bool,
    replaced: bool,
    planned: bool,
    sent_back: bool,
    idled_at_deadline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Then {
    Report,
    CarryOn,
    Nothing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Message,
    Allocate,
    NothingCompatible,
    CompatibleAgain,
    DriverWaitDeadline,
    StartTurn,
    EndTurn(Then),
    EndLastTurn,
    Fail,
    Replace,
    PlanHandoff,
    Accept,
    SendBack,
    Retire,
}

fn item(n: u8) -> InboxItemId {
    InboxItemId::from_u128(u128::from(n))
}

fn driver_id(i: usize) -> DriverId {
    DriverId::from_u128(i as u128 + 1)
}

impl State {
    fn task_open(&self) -> bool {
        matches!(self.task, TaskState::InProgress(_)) && !self.gave_up
    }

    fn has_work(&self) -> bool {
        self.inbox.waiting().next().is_some() || self.task_open()
    }

    fn live_current(&self) -> Option<usize> {
        self.current.filter(|i| self.drivers[*i].is_live())
    }

    fn reported_with(&self, outcome: impl Fn(&Outcome) -> bool) -> bool {
        match &self.task {
            TaskState::InReview(_, r) | TaskState::Promoting(_, r) | TaskState::Done(_, r) => {
                outcome(&r.outcome)
            }
            _ => false,
        }
    }

    fn done_with(&self, outcome: impl Fn(&Outcome) -> bool) -> bool {
        matches!(self.task, TaskState::Done(..)) && self.reported_with(outcome)
    }

    fn deliver(&mut self, what: InboxItem) -> Option<()> {
        self.delivered += 1;
        self.idled_at_deadline = false;
        let id = item(self.delivered);
        let was_waiting_for_driver =
            matches!(self.agent, AgentState::Waiting(Waiting::ForDriver(_)));
        self.agent = agent::next(&self.agent, &AgentEvent::Delivered { item: id, what }).ok()?;
        self.inbox.deliver(id).ok()?;
        if was_waiting_for_driver {
            self.end_driver_wait();
        }
        Some(())
    }

    /// The wait for a driver is over, so its deadline job is cancelled, if it has not fired.
    fn end_driver_wait(&mut self) {
        if let Some(job) = self.driver_wait
            && let Ok(cancelled) = job::next(&job, &JobEvent::Cancelled { by: Actor::Daemon })
        {
            self.driver_wait = Some(cancelled);
        }
    }

    fn go_idle_if_done(&mut self) {
        if self.inbox.is_empty()
            && !self.task_open()
            && let Ok(idle) = agent::next(&self.agent, &AgentEvent::WentIdle)
        {
            self.agent = idle;
        }
    }
}

impl Takeover {
    /// The daemon reports on the assignee's behalf. If the task machine refuses, nothing else does
    /// it.
    fn daemon_reports(&self, s: &mut State, failure: TaskFailure) {
        s.gave_up = true;
        let report = Report {
            outcome: Outcome::Failed(failure),
            summary: String::new(),
        };
        if let Ok(task) = (self.m.task)(&s.task, &TaskEvent::Reported(report)) {
            s.task = task;
            s.reported = true;
        }
    }

    fn fence(&self, s: &mut State, i: usize, event: DriverEvent) -> Option<()> {
        s.drivers[i] = (self.m.driver)(&s.drivers[i], &event).ok()?;
        s.current = None;
        if s.in_turn.remove(&i).is_some() {
            s.inbox.turn_lost(driver_id(i));
        }
        Some(())
    }
}

impl Model for Takeover {
    type State = State;
    type Action = Step;

    fn init_states(&self) -> Vec<State> {
        let assignment = Assignment {
            delegator: Party::Agent(DELEGATOR),
            assignee: AgentId::from_u128(1),
            effort: Effort::High,
        };
        let mut s = State {
            agent: AgentState::INITIAL,
            task: TaskState::InProgress(assignment),
            drivers: Vec::new(),
            current: None,
            inbox: Inbox::default(),
            delivered: 0,
            handled: BTreeSet::new(),
            in_turn: BTreeMap::new(),
            failures: 0,
            in_a_row: 0,
            gave_up: false,
            nudges: 0,
            driver_wait: None,
            reported: false,
            message_sent: false,
            no_driver_once: false,
            replaced: false,
            planned: false,
            sent_back: false,
            idled_at_deadline: false,
        };
        s.deliver(InboxItem::Task { task: TASK })
            .expect("a planned agent accepts its first task");
        vec![s]
    }

    fn actions(&self, _: &State, actions: &mut Vec<Step>) {
        actions.extend([
            Step::Message,
            Step::Allocate,
            Step::NothingCompatible,
            Step::CompatibleAgain,
            Step::DriverWaitDeadline,
            Step::StartTurn,
            Step::EndTurn(Then::Report),
            Step::EndTurn(Then::CarryOn),
            Step::EndTurn(Then::Nothing),
            Step::EndLastTurn,
            Step::Fail,
            Step::Replace,
            Step::PlanHandoff,
            Step::Accept,
            Step::SendBack,
            Step::Retire,
        ]);
    }

    fn next_state(&self, s: &State, step: Step) -> Option<State> {
        let mut s = s.clone();
        let needs_driver =
            s.agent == AgentState::Working && s.live_current().is_none() && s.has_work();
        match step {
            Step::Message if !s.message_sent => {
                s.message_sent = true;
                s.deliver(InboxItem::Message {
                    message: crate::id::MessageId::from_u128(1),
                })?;
            }
            Step::Allocate if needs_driver => {
                s.drivers.push(DriverState::INITIAL);
                s.current = Some(s.drivers.len() - 1);
            }
            Step::NothingCompatible if needs_driver && !s.no_driver_once => {
                s.no_driver_once = true;
                s.agent = agent::next(
                    &s.agent,
                    &AgentEvent::NoDriver(vec![Incompatible::NothingAllowed]),
                )
                .ok()?;
                s.driver_wait = Some(JobState::initial(&Schedule::Once {
                    at: Timestamp::from_millis(1),
                }));
            }
            Step::CompatibleAgain => {
                s.agent = agent::next(&s.agent, &AgentEvent::DriverFound).ok()?;
                s.end_driver_wait();
            }
            Step::DriverWaitDeadline => {
                let job = s.driver_wait?;
                s.driver_wait = Some(
                    job::next(
                        &job,
                        &JobEvent::Fired {
                            at: Timestamp::from_millis(1),
                        },
                    )
                    .ok()?,
                );
                if s.task_open() {
                    self.daemon_reports(
                        &mut s,
                        TaskFailure::NoDriver(vec![Incompatible::NothingAllowed]),
                    );
                }
                // Items it has not handled stay in its inbox, for the turn after whatever next
                // wakes it.
                s.agent = agent::next(&s.agent, &AgentEvent::WentIdle).ok()?;
                s.idled_at_deadline = true;
            }
            Step::StartTurn => {
                let i = s.live_current()?;
                let fresh = s.drivers[i] == DriverState::Starting;
                let items: Vec<_> = s.inbox.waiting().collect();
                if s.agent != AgentState::Working || (items.is_empty() && !(fresh && s.task_open()))
                {
                    return None;
                }
                (self.m.give)(&mut s.inbox, &items, driver_id(i)).ok()?;
                s.drivers[i] = (self.m.driver)(
                    &s.drivers[i],
                    &DriverEvent::TurnStarted {
                        given: items.clone(),
                    },
                )
                .ok()?;
                s.in_turn.insert(i, items);
            }
            Step::EndTurn(then) => {
                let i = s.live_current()?;
                if s.drivers[i] != DriverState::Running {
                    return None;
                }
                s.drivers[i] = (self.m.driver)(&s.drivers[i], &DriverEvent::TurnEnded).ok()?;
                s.inbox.turn_completed(driver_id(i));
                s.handled.extend(s.in_turn.remove(&i).unwrap_or_default());
                s.in_a_row = 0;
                match then {
                    Then::Report if s.task_open() => {
                        let report = Report {
                            outcome: Outcome::Completed,
                            summary: String::new(),
                        };
                        s.task = (self.m.task)(&s.task, &TaskEvent::Reported(report)).ok()?;
                        s.reported = true;
                    }
                    Then::CarryOn if s.task_open() => {
                        s.nudges += 1;
                        let turns = u32::from(s.nudges);
                        if s.nudges >= NUDGE_LIMIT {
                            self.daemon_reports(&mut s, TaskFailure::NoProgress { turns });
                        } else {
                            s.deliver(InboxItem::Nudge { task: TASK, turns })?;
                        }
                    }
                    Then::Nothing if !s.task_open() => {}
                    _ => return None,
                }
                s.go_idle_if_done();
            }
            Step::EndLastTurn => {
                let i = s.live_current()?;
                if !matches!(s.drivers[i], DriverState::HandingOff(_)) {
                    return None;
                }
                self.fence(&mut s, i, DriverEvent::TurnEnded)?;
            }
            Step::Fail if s.failures < MAX_FAILURES => {
                let i = s.live_current()?;
                self.fence(&mut s, i, DriverEvent::Failed(Failure::StreamBroke))?;
                s.failures += 1;
                if s.task_open() {
                    s.in_a_row += 1;
                    if u32::from(s.in_a_row) >= FAILURES_BEFORE_GIVING_UP {
                        self.daemon_reports(
                            &mut s,
                            TaskFailure::DriverFailures(vec![Failure::StreamBroke; 3]),
                        );
                    }
                }
            }
            Step::Replace if !s.replaced && s.task_open() => {
                let i = s.live_current()?;
                s.replaced = true;
                self.fence(
                    &mut s,
                    i,
                    DriverEvent::Stopped(StopReason::ReplacedByBuilder),
                )?;
            }
            Step::PlanHandoff if !s.planned && s.task_open() => {
                let i = s.live_current()?;
                s.planned = true;
                s.drivers[i] = (self.m.driver)(
                    &s.drivers[i],
                    &DriverEvent::HandoffStarted(PlannedReason::ContextFull),
                )
                .ok()?;
            }
            Step::Accept if s.reported => {
                s.task = (self.m.task)(&s.task, &TaskEvent::Reviewed(Verdict::Accept)).ok()?;
                s.task = (self.m.task)(
                    &s.task,
                    &TaskEvent::Merged {
                        commit: crate::git::CommitId(String::new()),
                    },
                )
                .ok()?;
            }
            Step::SendBack if s.reported && !s.sent_back => {
                s.task = (self.m.task)(
                    &s.task,
                    &TaskEvent::Reviewed(Verdict::SendBack {
                        note: String::new(),
                        effort: None,
                    }),
                )
                .ok()?;
                s.sent_back = true;
                s.reported = false;
                s.gave_up = false;
                s.in_a_row = 0;
                s.nudges = 0;
                s.deliver(InboxItem::Task { task: TASK })?;
            }
            Step::Retire if s.agent == AgentState::Idle => {
                let i = s.live_current()?;
                s.drivers[i] = (self.m.driver)(
                    &s.drivers[i],
                    &DriverEvent::Retired(RetireReason::WarmTooLong),
                )
                .ok()?;
                s.current = None;
            }
            _ => return None,
        }
        Some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("at most one driver acts for the agent", |_, s: &State| {
                s.drivers.iter().filter(|d| d.is_live()).count() <= 1
            }),
            Property::always(
                "every delivered item is waiting or was handled by a completed turn",
                |_, s: &State| {
                    let unhandled: BTreeSet<_> = s.inbox.unhandled().collect();
                    (1..=s.delivered)
                        .map(item)
                        .all(|i| unhandled.contains(&i) != s.handled.contains(&i))
                },
            ),
            Property::always(
                "every delegated task ends in a report to its delegator",
                |m, s: &State| {
                    !is_terminal(m, s)
                        || (s.reported && !matches!(s.task, TaskState::InProgress(_)))
                },
            ),
            Property::always(
                "every inbox item is handled in the end, or waits for the agent's next wake",
                |m, s: &State| {
                    !is_terminal(m, s)
                        || s.inbox.is_empty()
                        || (s.idled_at_deadline && s.agent == AgentState::Idle)
                },
            ),
            Property::always("the agent ends idle, not waiting", |m, s: &State| {
                !is_terminal(m, s) || s.agent == AgentState::Idle
            }),
            Property::sometimes("the task is done after a takeover", |_, s: &State| {
                s.failures > 0 && s.done_with(|o| *o == Outcome::Completed)
            }),
            Property::sometimes(
                "the task is done after a planned handoff",
                |_, s: &State| s.planned && s.done_with(|o| *o == Outcome::Completed),
            ),
            Property::sometimes(
                "the task is reported failed after three failures",
                |_, s: &State| {
                    s.reported_with(|o| {
                        matches!(o, Outcome::Failed(TaskFailure::DriverFailures(_)))
                    })
                },
            ),
            Property::sometimes(
                "the task is reported failed with no driver",
                |_, s: &State| {
                    s.reported_with(|o| matches!(o, Outcome::Failed(TaskFailure::NoDriver(_))))
                },
            ),
            Property::sometimes(
                "the agent goes idle when the wait for a driver runs out",
                |_, s: &State| s.idled_at_deadline,
            ),
        ]
    }
}

/// Bug: a failure report is refused while a task is in progress, as if only success could be
/// reported.
fn rejects_failure_reports(s: &TaskState, e: &TaskEvent) -> Result<TaskState, Rejected> {
    match (s, e) {
        (
            TaskState::InProgress(_),
            TaskEvent::Reported(Report {
                outcome: Outcome::Failed(_),
                ..
            }),
        ) => Err(Rejected {
            machine: crate::MachineKind::Task,
            state: s.name(),
            event: e.name(),
        }),
        _ => task::next(s, e),
    }
}

/// Bug: inbox items count as handled as soon as they are given to a turn.
fn handled_when_given(
    inbox: &mut Inbox,
    items: &[InboxItemId],
    driver: DriverId,
) -> Result<(), InboxError> {
    inbox.give(items, driver)?;
    inbox.turn_completed(driver);
    Ok(())
}

/// Bug: stopping a driver mid-turn is accepted but leaves it running until its turn ends.
fn stop_waits_for_turn(s: &DriverState, e: &DriverEvent) -> Result<DriverState, Rejected> {
    match (s, e) {
        (DriverState::Running, DriverEvent::Stopped(_)) => Ok(DriverState::Running),
        _ => driver::next(s, e),
    }
}

#[test]
fn delegated_work_survives_driver_failures() {
    prove("takeover", Takeover { m: REAL });
}

#[test]
fn a_task_that_refuses_failure_reports_is_caught() {
    refute(
        "takeover with bug rejects_failure_reports",
        Takeover {
            m: Machines {
                task: rejects_failure_reports,
                ..REAL
            },
        },
        "every delegated task ends in a report to its delegator",
    );
}

#[test]
fn an_inbox_that_handles_items_when_given_is_caught() {
    refute(
        "takeover with bug handled_when_given",
        Takeover {
            m: Machines {
                give: handled_when_given,
                ..REAL
            },
        },
        "every delivered item is waiting or was handled by a completed turn",
    );
}

#[test]
fn a_stop_that_leaves_the_driver_running_is_caught() {
    refute(
        "takeover with bug stop_waits_for_turn",
        Takeover {
            m: Machines {
                driver: stop_waits_for_turn,
                ..REAL
            },
        },
        "at most one driver acts for the agent",
    );
}
