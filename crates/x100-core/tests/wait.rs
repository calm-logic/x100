//! Which events end a wait.

use x100_core::action::ActionEvent;
use x100_core::agent::AgentEvent;
use x100_core::event::ProjectEvent;
use x100_core::id::{ActionId, AgentId, JobId, MessageId, TaskId, WaitId};
use x100_core::job::JobEvent;
use x100_core::message::{Message, Route};
use x100_core::party::{Actor, Party};
use x100_core::task::{Outcome, Report, TaskEvent};
use x100_core::time::Timestamp;
use x100_core::wait::{Wait, WaitEnd, WaitOn};

fn wait(on: WaitOn) -> Wait {
    Wait {
        id: WaitId::from_u128(1),
        on,
        deadline: Timestamp::from_millis(1_000),
        deadline_job: JobId::from_u128(99),
    }
}

fn job(n: u128, event: JobEvent) -> ProjectEvent {
    ProjectEvent::Job {
        job: JobId::from_u128(n),
        event,
    }
}

const FIRED: JobEvent = JobEvent::Fired {
    at: Timestamp::from_millis(5),
};

#[test]
fn every_wait_expires_when_its_deadline_job_fires() {
    for on in [
        WaitOn::Job(JobId::from_u128(1)),
        WaitOn::Report(TaskId::from_u128(1)),
        WaitOn::Reply {
            message: MessageId::from_u128(1),
            from: Party::Builder,
        },
        WaitOn::Decision(ActionId::from_u128(1)),
    ] {
        assert_eq!(wait(on).ended_by(&job(99, FIRED)), Some(WaitEnd::Expired));
    }
}

#[test]
fn a_wait_on_a_job_ends_when_it_fires_or_can_no_longer_fire() {
    let w = wait(WaitOn::Job(JobId::from_u128(1)));
    assert_eq!(w.ended_by(&job(1, FIRED)), Some(WaitEnd::Satisfied));
    assert_eq!(
        w.ended_by(&job(1, JobEvent::Cancelled { by: Actor::Builder })),
        Some(WaitEnd::CannotHappen)
    );
    assert_eq!(w.ended_by(&job(2, FIRED)), None);
}

#[test]
fn a_wait_on_a_report_ends_when_the_task_is_reported() {
    let w = wait(WaitOn::Report(TaskId::from_u128(1)));
    let reported = |n| ProjectEvent::Task {
        task: TaskId::from_u128(n),
        event: TaskEvent::Reported(Report {
            outcome: Outcome::Completed,
            summary: String::new(),
        }),
    };
    assert_eq!(w.ended_by(&reported(1)), Some(WaitEnd::Satisfied));
    assert_eq!(w.ended_by(&reported(2)), None);
}

#[test]
fn a_wait_on_a_reply_ends_with_the_reply_or_when_the_one_asked_retires() {
    let asked = AgentId::from_u128(7);
    let w = wait(WaitOn::Reply {
        message: MessageId::from_u128(1),
        from: Party::Agent(asked),
    });
    let reply = ProjectEvent::MessageSent(Message {
        id: MessageId::from_u128(2),
        route: Route::Between {
            from: asked,
            to: AgentId::from_u128(8),
        },
        body: "yes".into(),
        attachments: vec![],
        in_reply_to: Some(MessageId::from_u128(1)),
    });
    assert_eq!(w.ended_by(&reply), Some(WaitEnd::Satisfied));
    let retired = ProjectEvent::Agent {
        agent: asked,
        event: AgentEvent::Retired,
    };
    assert_eq!(w.ended_by(&retired), Some(WaitEnd::CannotHappen));
}

#[test]
fn a_wait_on_a_decision_ends_when_the_action_is_decided() {
    let w = wait(WaitOn::Decision(ActionId::from_u128(1)));
    let action = |event| ProjectEvent::Action {
        action: ActionId::from_u128(1),
        event,
    };
    for decided in [
        ActionEvent::Allowed,
        ActionEvent::Approved,
        ActionEvent::Denied,
    ] {
        assert_eq!(w.ended_by(&action(decided)), Some(WaitEnd::Satisfied));
    }
    assert_eq!(w.ended_by(&action(ActionEvent::Held)), None);
}
