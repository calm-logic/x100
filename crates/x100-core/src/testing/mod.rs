//! Realistic, valid logs for tests, here and in later crates. Behind the `testing` feature.
//!
//! [`LogBuilder`] writes a project's log the way the daemon would, one tool call or routine at a
//! time, and folds each event as it goes, so a scenario that breaks a rule fails at the event that
//! breaks it. [`fernleaf`] is a whole project built with it, and [`daemon_log`] the daemon's log
//! that goes with it.

mod daemon;
mod fernleaf;
mod flows;
mod tools;

use std::time::Duration;

pub use daemon::{CLAUDE, CODEX, FERNLEAF, MACHINE, daemon_log, gpt, opus, sonnet};
pub use fernleaf::{Fernleaf, START, deploy_skill, fernleaf};

use crate::action::{
    Action, ActionEvent, ActionKind, Classification, ClassifiedBy, Decision, Request, Risk,
};
use crate::agent::AgentEvent;
use crate::driver::{ContextFill, Driver, DriverEvent, Purpose, StartReason, View};
use crate::effort::Effort;
use crate::event::{Envelope, ProjectEvent, Seq};
use crate::fold::ProjectState;
use crate::git::CommitId;
use crate::harness::HarnessModel;
use crate::id::{ActionId, AgentId, DriverId, InboxItemId, JobId, TaskId, Ulid, WaitId};
use crate::inbox::InboxItem;
use crate::job::{Job, JobEvent, ProjectJob, ProjectRoutine, Schedule};
use crate::ledger::{Charge, ChargeSource, CostBasis, Payer};
use crate::money::{Money, Percent, Usage};
use crate::party::Actor;
use crate::task::{Progress, TaskEvent};
use crate::time::Timestamp;
use crate::tool::ToolCall;
use crate::wait::{Wait, WaitEnd, WaitOn};

/// How long the daemon gives a wait with no deadline of its own, in these logs.
const WAIT_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);

/// Writes a valid project log, folding each event as it is added.
pub struct LogBuilder {
    state: ProjectState,
    log: Vec<Envelope<ProjectEvent>>,
    now: Timestamp,
    serial: u128,
}

impl LogBuilder {
    pub fn new(start: Timestamp) -> Self {
        Self {
            state: ProjectState::default(),
            log: Vec::new(),
            now: start,
            serial: 0,
        }
    }

    pub fn state(&self) -> &ProjectState {
        &self.state
    }

    pub fn finish(self) -> Vec<Envelope<ProjectEvent>> {
        self.log
    }

    pub fn now(&self) -> Timestamp {
        self.now
    }

    pub fn advance(&mut self, by: Duration) {
        self.now = self.now + by;
    }

    /// A fresh id of any kind, ordered by time like the daemon's.
    pub fn id<T>(&mut self, make: fn(Ulid) -> T) -> T {
        self.serial += 1;
        make(Ulid::from_parts(self.now.as_millis(), self.serial))
    }

    /// Appends one event a second after the last, and folds it.
    ///
    /// # Panics
    ///
    /// If the fold refuses it: the scenario broke a rule.
    pub fn push(&mut self, by: Actor, event: ProjectEvent) {
        self.advance(Duration::from_secs(1));
        let envelope = Envelope {
            seq: Seq(self.log.len() as u64 + 1),
            at: self.now,
            by,
            event,
        };
        if let Err(e) = self.state.apply(&envelope) {
            panic!("{e}\n{envelope:#?}");
        }
        self.log.push(envelope);
    }

    fn agent_event(&mut self, agent: AgentId, event: AgentEvent) {
        self.push(Actor::Daemon, ProjectEvent::Agent { agent, event });
    }

    fn driver_event(&mut self, driver: DriverId, event: DriverEvent) {
        self.push(Actor::Daemon, ProjectEvent::Driver { driver, event });
    }

    fn task_event(&mut self, by: Actor, task: TaskId, event: TaskEvent) {
        self.push(by, ProjectEvent::Task { task, event });
    }

    fn action_event(&mut self, by: Actor, action: ActionId, event: ActionEvent) {
        self.push(by, ProjectEvent::Action { action, event });
    }

    fn agent_of(&self, driver: DriverId) -> AgentId {
        self.state
            .driver(driver)
            .expect("a known driver")
            .driver
            .agent
    }

    pub fn deliver(&mut self, agent: AgentId, what: InboxItem) -> InboxItemId {
        let item = self.id(InboxItemId::new);
        self.agent_event(agent, AgentEvent::Delivered { item, what });
        item
    }

    /// The router's choice of a driver for `agent`.
    pub fn start_driver(
        &mut self,
        agent: AgentId,
        on: HarnessModel,
        effort: Effort,
        why: StartReason,
    ) -> DriverId {
        self.start(agent, on, effort, Purpose::Work, why)
    }

    fn start(
        &mut self,
        agent: AgentId,
        on: HarnessModel,
        effort: Effort,
        purpose: Purpose,
        why: StartReason,
    ) -> DriverId {
        let id = self.id(DriverId::new);
        let driver = Driver {
            id,
            agent,
            harness: on.harness,
            model: on.model,
            effort,
            machine: MACHINE,
            purpose,
            view: View::Headless,
            why,
        };
        self.push(Actor::Daemon, ProjectEvent::DriverStarted(driver));
        id
    }

    /// One turn on `driver`: it is given every item waiting in its agent's inbox, does `work`,
    /// and is charged `cost` micro-dollars when the turn ends.
    pub fn turn(&mut self, driver: DriverId, cost: u64, work: impl FnOnce(&mut Self)) {
        let record = self.state.driver(driver).expect("a known driver");
        let agent = record.driver.agent;
        let lab = record.driver.purpose != Purpose::Work;
        let turns = u64::from(record.turns) + 1;
        let given = if lab {
            Vec::new()
        } else {
            let a = self.state.agent(agent).expect("a known agent");
            a.inbox.waiting().collect()
        };
        self.driver_event(driver, DriverEvent::TurnStarted { given });
        work(self);
        let payer = if lab {
            Payer::Lab
        } else {
            let team = &self.state.project().expect("created").team;
            team.payer(agent).expect("on the team")
        };
        let basis = if record_harness(self, driver) == CODEX {
            CostBasis::Listed
        } else {
            CostBasis::Reported
        };
        self.push(
            Actor::Daemon,
            ProjectEvent::Charged(Charge {
                payer,
                source: ChargeSource::Turn { driver, agent },
                usage: Usage {
                    input: cost / 4,
                    output: cost / 20,
                    cache_read: cost,
                    cache_write_short: cost / 10,
                    cache_write_long: 0,
                },
                cost: Money::from_micros(cost),
                basis,
            }),
        );
        self.driver_event(
            driver,
            DriverEvent::ContextReported(ContextFill {
                used: (turns * 18_000).min(200_000),
                window: 200_000,
            }),
        );
        self.driver_event(driver, DriverEvent::TurnEnded);
    }

    /// A daemon tool call the gate lets through: proposed, classified by the rules as low risk,
    /// allowed, carried out with `effects`, and marked done.
    pub fn tool(
        &mut self,
        driver: DriverId,
        call: ToolCall,
        effects: impl FnOnce(&mut Self),
    ) -> ActionId {
        let agent = self.agent_of(driver);
        let id = self.propose(agent, Request::Tool { driver, call });
        self.classify(
            id,
            ActionKind::Other,
            Risk::Low,
            ClassifiedBy::Rules,
            "Routine.",
        );
        self.action_event(Actor::Daemon, id, ActionEvent::Allowed);
        effects(self);
        self.action_event(Actor::Daemon, id, ActionEvent::CarriedOut);
        id
    }

    fn propose(&mut self, agent: AgentId, request: Request) -> ActionId {
        let id = self.id(ActionId::new);
        let by = match request {
            Request::Merge { .. } => Actor::Daemon,
            _ => Actor::Agent(agent),
        };
        self.push(
            by,
            ProjectEvent::ActionProposed(Action { id, agent, request }),
        );
        id
    }

    fn classify(
        &mut self,
        id: ActionId,
        kind: ActionKind,
        risk: Risk,
        by: ClassifiedBy,
        why: &str,
    ) {
        let c = Classification {
            kind,
            risk,
            by,
            why: why.into(),
        };
        self.action_event(Actor::Daemon, id, ActionEvent::Classified(c));
    }

    /// An action the gate holds for the builder.
    pub fn held(
        &mut self,
        agent: AgentId,
        request: Request,
        kind: ActionKind,
        by: ClassifiedBy,
        why: &str,
    ) -> ActionId {
        let id = self.propose(agent, request);
        self.classify(id, kind, Risk::High, by, why);
        self.action_event(Actor::Daemon, id, ActionEvent::Held);
        id
    }

    /// The builder's Approve or Deny, which ends a wait on it and reaches the agent's inbox.
    pub fn decide(&mut self, action: ActionId, decision: Decision) {
        let event = match decision {
            Decision::Approved => ActionEvent::Approved,
            Decision::Denied => ActionEvent::Denied,
        };
        self.action_event(Actor::Builder, action, event);
        let agent = self
            .state
            .action(action)
            .expect("a known action")
            .action
            .agent;
        self.satisfy(agent, WaitOn::Decision(action));
        self.deliver(agent, InboxItem::Decision { action, decision });
    }

    /// Ends every wait of `agent` on `on`, and cancels each one's deadline.
    pub fn satisfy(&mut self, agent: AgentId, on: WaitOn) {
        let waits: Vec<Wait> = self
            .state
            .agent(agent)
            .map(|a| a.waits.values().filter(|w| w.on == on).cloned().collect())
            .unwrap_or_default();
        for w in waits {
            self.agent_event(
                agent,
                AgentEvent::WaitEnded {
                    wait: w.id,
                    end: WaitEnd::Satisfied,
                },
            );
            self.push(
                Actor::Daemon,
                ProjectEvent::Job {
                    job: w.deadline_job,
                    event: JobEvent::Cancelled { by: Actor::Daemon },
                },
            );
        }
    }

    /// `wait`, with the progress note it requires and the deadline job the daemon schedules.
    pub fn wait(&mut self, driver: DriverId, task: TaskId, on: WaitOn, note: &str, done: u8) {
        let agent = self.agent_of(driver);
        let progress = Progress {
            note: note.into(),
            done: Some(Percent::new(done).expect("a percentage")),
        };
        let call = ToolCall::Wait {
            on,
            deadline: None,
            task,
            progress: progress.clone(),
        };
        self.tool(driver, call, |b| {
            b.task_event(
                Actor::Agent(agent),
                task,
                TaskEvent::ProgressNoted(progress),
            );
            let id = b.id(WaitId::new);
            let deadline = b.now() + WAIT_DEADLINE;
            let deadline_job = b.schedule_routine(
                Schedule::Once { at: deadline },
                ProjectRoutine::WaitDeadline { agent, wait: id },
            );
            let wait = Wait {
                id,
                on,
                deadline,
                deadline_job,
            };
            b.agent_event(agent, AgentEvent::WaitOpened(wait));
        });
    }

    pub fn schedule_routine(&mut self, when: Schedule, routine: ProjectRoutine) -> JobId {
        let id = self.id(JobId::new);
        let job = Job {
            id,
            when,
            does: ProjectJob::Routine(routine),
        };
        self.push(Actor::Daemon, ProjectEvent::JobScheduled(job));
        id
    }
}

fn record_harness(b: &LogBuilder, driver: DriverId) -> crate::id::HarnessId {
    b.state
        .driver(driver)
        .expect("a known driver")
        .driver
        .harness
}

/// A made-up commit hash, readable in a test's output.
pub fn commit(what: &str) -> CommitId {
    CommitId(format!("{what:0>40}"))
}
