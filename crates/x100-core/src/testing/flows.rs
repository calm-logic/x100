//! The daemon's flows for delegated work, messages and jobs, as the builder writes them.

use std::time::Duration;

use crate::action::{ActionEvent, ActionKind, ClassifiedBy, Request, Risk};
use crate::agent::{Agent, AgentEvent, Position};
use crate::driver::DriverEvent;
use crate::effort::Effort;
use crate::event::ProjectEvent;
use crate::id::{ActionId, AgentId, DriverId, JobId, MessageId, TaskId};
use crate::inbox::InboxItem;
use crate::job::{JobEvent, ProjectJob, ProjectRoutine, Schedule};
use crate::message::{Message, Route};
use crate::money::Percent;
use crate::party::{Actor, Party};
use crate::task::{
    Assignment, Opening as TaskOpening, Outcome, Progress, Report, Task, TaskEvent, Verdict,
};
use crate::testing::{LogBuilder, commit};
use crate::tool::{Delegated, Delegatee, Recipient, ToolCall};
use crate::wait::WaitOn;

impl LogBuilder {
    /// `update_task`.
    pub fn update(&mut self, driver: DriverId, task: TaskId, note: &str, done: u8) {
        let agent = self.agent_of(driver);
        let progress = Progress {
            note: note.into(),
            done: Some(Percent::new(done).expect("a percentage")),
        };
        let call = ToolCall::UpdateTask {
            task,
            progress: progress.clone(),
            effort: None,
        };
        self.tool(driver, call, |b| {
            b.task_event(
                Actor::Agent(agent),
                task,
                TaskEvent::ProgressNoted(progress),
            );
        });
    }

    /// `message` from an agent to another agent or the builder. A reply ends the wait on it and,
    /// for an agent, reaches its inbox.
    pub fn message(
        &mut self,
        driver: DriverId,
        to: Recipient,
        body: &str,
        in_reply_to: Option<MessageId>,
    ) -> MessageId {
        let from = self.agent_of(driver);
        let call = ToolCall::Message {
            to,
            body: body.into(),
            attachments: vec![],
            in_reply_to,
        };
        let id = self.id(MessageId::new);
        let route = match to {
            Recipient::Agent(to) => Route::Between { from, to },
            Recipient::Builder => Route::CeoToBuilder,
        };
        self.tool(driver, call, |b| {
            let m = Message {
                id,
                route,
                body: body.into(),
                attachments: vec![],
                in_reply_to,
            };
            b.push(Actor::Agent(from), ProjectEvent::MessageSent(m));
        });
        if let Recipient::Agent(to) = to {
            if let Some(original) = in_reply_to {
                self.satisfy(
                    to,
                    WaitOn::Reply {
                        message: original,
                        from: Party::Agent(from),
                    },
                );
            }
            self.deliver(to, InboxItem::Message { message: id });
        }
        id
    }

    /// The builder's message to the CEO, which reaches the CEO's inbox.
    pub fn builder_says(&mut self, body: &str, in_reply_to: Option<MessageId>) -> MessageId {
        let id = self.id(MessageId::new);
        let m = Message {
            id,
            route: Route::BuilderToCeo,
            body: body.into(),
            attachments: vec![],
            in_reply_to,
        };
        self.push(Actor::Builder, ProjectEvent::MessageSent(m));
        let ceo = self.state.project().expect("created").ceo;
        if let Some(original) = in_reply_to {
            self.satisfy(
                ceo,
                WaitOn::Reply {
                    message: original,
                    from: Party::Builder,
                },
            );
        }
        self.deliver(ceo, InboxItem::Message { message: id });
        id
    }

    /// `delegate` to an existing child, whose branch is refreshed from the delegator's first.
    pub fn delegate(
        &mut self,
        driver: DriverId,
        to: AgentId,
        title: &str,
        ask: &str,
        effort: Effort,
    ) -> TaskId {
        let from = self.agent_of(driver);
        let call = delegate_call(Delegatee::Child(to), title, ask, effort);
        let mut task = None;
        self.tool(driver, call, |b| {
            b.push(
                Actor::Daemon,
                ProjectEvent::BranchRefreshed {
                    agent: to,
                    from: Party::Agent(from),
                    commit: commit("refresh"),
                },
            );
            task = Some(b.open_delegated(Party::Agent(from), to, title, ask, effort));
        });
        task.expect("opened")
    }

    /// `delegate` to a new worker, which starts it under the delegator.
    pub fn delegate_new(
        &mut self,
        driver: DriverId,
        role: &str,
        title: &str,
        ask: &str,
        effort: Effort,
    ) -> (TaskId, AgentId) {
        let from = self.agent_of(driver);
        let charter = format!("{role} for {title}.");
        let to = Delegatee::NewWorker {
            role: role.into(),
            charter: charter.clone(),
            effort: None,
        };
        let worker = self.id(AgentId::new);
        let mut task = None;
        self.tool(driver, delegate_call(to, title, ask, effort), |b| {
            let agent = Agent {
                id: worker,
                role: role.into(),
                charter,
                position: Position::Worker {
                    parent: from,
                    effort: None,
                },
                appearance: b.child_look(from, worker),
            };
            b.push(Actor::Agent(from), ProjectEvent::AgentAdded(agent));
            task = Some(b.open_delegated(Party::Agent(from), worker, title, ask, effort));
        });
        (task.expect("opened"), worker)
    }

    /// Opens a task delegated by `from` and puts it in the assignee's inbox.
    pub fn open_delegated(
        &mut self,
        from: Party,
        to: AgentId,
        title: &str,
        ask: &str,
        effort: Effort,
    ) -> TaskId {
        let id = self.id(TaskId::new);
        let task = Task {
            id,
            title: title.into(),
            ask: ask.into(),
            part: Some("Website".into()),
            raised_by: from,
            attachments: vec![],
        };
        let opening = TaskOpening::Delegated(Assignment {
            delegator: from,
            assignee: to,
            effort,
        });
        self.push(from.into(), ProjectEvent::TaskOpened { task, opening });
        self.deliver(to, InboxItem::Task { task: id });
        id
    }

    /// `report` on a task completed. The delegator's wait on it ends and the report reaches it.
    pub fn report(&mut self, driver: DriverId, task: TaskId, summary: &str) {
        let agent = self.agent_of(driver);
        let call = ToolCall::Report {
            task,
            outcome: crate::tool::AgentOutcome::Completed,
            summary: summary.into(),
        };
        let report = Report {
            outcome: Outcome::Completed,
            summary: summary.into(),
        };
        self.tool(driver, call, |b| {
            b.task_event(Actor::Agent(agent), task, TaskEvent::Reported(report));
        });
        let delegator = self.delegator(task);
        if let Party::Agent(d) = delegator {
            self.satisfy(d, WaitOn::Report(task));
            self.deliver(d, InboxItem::Report { task });
        }
    }

    fn delegator(&self, task: TaskId) -> Party {
        let t = self.state.task(task).expect("a known task");
        t.state.assignment().expect("assigned").delegator
    }

    fn assignee(&self, task: TaskId) -> AgentId {
        let t = self.state.task(task).expect("a known task");
        t.state.assignment().expect("assigned").assignee
    }

    /// `review` accepting a report. The merge is left to [`LogBuilder::merge`], so a task can be
    /// left Promoting.
    pub fn accept(&mut self, driver: DriverId, task: TaskId) -> ActionId {
        let agent = self.agent_of(driver);
        let call = ToolCall::Review {
            task,
            verdict: Verdict::Accept,
        };
        self.tool(driver, call, |b| {
            b.task_event(
                Actor::Agent(agent),
                task,
                TaskEvent::Reviewed(Verdict::Accept),
            );
        });
        let merge = self.propose(agent, Request::Merge { task });
        self.classify(
            merge,
            ActionKind::Merge,
            Risk::Low,
            ClassifiedBy::Rules,
            "A merge.",
        );
        self.action_event(Actor::Daemon, merge, ActionEvent::Allowed);
        merge
    }

    /// The accepted task's branch merges, or conflicts and goes back to its assignee.
    pub fn merge(&mut self, merge: ActionId, task: TaskId, conflict: Option<&str>) {
        match conflict {
            None => {
                let commit = commit("merge");
                self.task_event(Actor::Daemon, task, TaskEvent::Merged { commit });
                self.action_event(Actor::Daemon, merge, ActionEvent::CarriedOut);
            }
            Some(conflict) => {
                let failed = TaskEvent::MergeFailed {
                    conflict: conflict.into(),
                };
                self.task_event(Actor::Daemon, task, failed);
                let error = ActionEvent::Failed {
                    error: conflict.into(),
                };
                self.action_event(Actor::Daemon, merge, error);
                let assignee = self.assignee(task);
                self.deliver(assignee, InboxItem::Task { task });
            }
        }
    }

    /// `review` sending a task back, asking for more effort where `effort` is set.
    pub fn send_back(
        &mut self,
        driver: DriverId,
        task: TaskId,
        note: &str,
        effort: Option<Effort>,
    ) {
        let agent = self.agent_of(driver);
        let verdict = Verdict::SendBack {
            note: note.into(),
            effort,
        };
        let call = ToolCall::Review {
            task,
            verdict: verdict.clone(),
        };
        self.tool(driver, call, |b| {
            b.task_event(Actor::Agent(agent), task, TaskEvent::Reviewed(verdict));
            if let Some(effort) = effort {
                b.task_event(Actor::Daemon, task, TaskEvent::EffortChanged(effort));
            }
        });
        let assignee = self.assignee(task);
        self.deliver(assignee, InboxItem::Task { task });
    }

    /// A job firing. It ends waits on it, wakes the agent it names, or delegates its task.
    pub fn fire(&mut self, job: JobId) -> Option<TaskId> {
        let at = self.now() + Duration::from_secs(1);
        self.push(
            Actor::Daemon,
            ProjectEvent::Job {
                job,
                event: JobEvent::Fired { at },
            },
        );
        let does = self.state.job(job).expect("a known job").job.does.clone();
        match does {
            ProjectJob::Wake { agent, note, .. } => {
                self.satisfy(agent, WaitOn::Job(job));
                self.deliver(agent, InboxItem::Job { job, note });
                None
            }
            ProjectJob::Delegate {
                scheduled_by,
                to,
                title,
                ask,
                effort,
            } => {
                let from = Party::Agent(scheduled_by);
                Some(self.open_delegated(from, to, &title, &ask, effort))
            }
            ProjectJob::Routine(_) => None,
        }
    }

    /// A warm driver's warm period runs out and it retires.
    pub fn warm_period_ends(&mut self, driver: DriverId, after: Duration) {
        let job = self.schedule_routine(
            Schedule::Once {
                at: self.now() + after,
            },
            ProjectRoutine::EndWarmPeriod { driver },
        );
        self.advance(after);
        self.fire(job);
        let retired = DriverEvent::Retired(crate::driver::RetireReason::WarmTooLong);
        self.driver_event(driver, retired);
    }

    pub fn went_idle(&mut self, agent: AgentId) {
        self.agent_event(agent, AgentEvent::WentIdle);
    }
}

fn delegate_call(to: Delegatee, title: &str, ask: &str, effort: Effort) -> ToolCall {
    ToolCall::Delegate {
        to,
        task: Delegated::New {
            title: title.into(),
            ask: ask.into(),
            attachments: vec![],
        },
        effort,
    }
}
