//! Every tool an agent calls, and every inbox item and routine, survives the trip to JSON and back.

use x100_core::action::{Action, Request};
use x100_core::driver::Incompatible;
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::Seq;
use x100_core::file::ContentHash;
use x100_core::id::*;
use x100_core::inbox::InboxItem;
use x100_core::job::{ProjectJob, ProjectRoutine, Schedule};
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope};
use x100_core::money::{Money, Percent};
use x100_core::party::Party;
use x100_core::process::ProcessName;
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::task::{Progress, Verdict};
use x100_core::team::{LeadSpec, TeamChange};
use x100_core::time::Timestamp;
use x100_core::tool::{
    AgentOutcome, Delegated, Delegatee, JobRequest, Proposal, Recipient, ToolCall,
};
use x100_core::trial::{EvalSet, TrialChange};
use x100_core::wait::{WaitEnd, WaitOn};

fn agent(n: u128) -> AgentId {
    AgentId::from_u128(n)
}

fn skill() -> SkillRef {
    SkillRef {
        scope: SkillScope::Shared,
        name: SkillName("deploy".into()),
    }
}

fn at(ms: u64) -> Timestamp {
    Timestamp::from_millis(ms)
}

/// Every tool an agent calls can be put in an action, and so in the log.
#[test]
fn every_tool_call_round_trips_inside_an_action() {
    let progress = Progress {
        note: "Schema done, API next.".into(),
        done: Some(Percent::new(40).unwrap()),
    };
    let range = EffortRange::new(Effort::Low, Effort::High).unwrap();
    let calls = vec![
        ToolCall::Delegate {
            to: Delegatee::Child(agent(2)),
            task: Delegated::Raised(TaskId::from_u128(3)),
            effort: Effort::Low,
        },
        ToolCall::Review {
            task: TaskId::from_u128(1),
            verdict: Verdict::SendBack {
                note: "Tests fail.".into(),
                effort: Some(Effort::High),
            },
        },
        ToolCall::Report {
            task: TaskId::from_u128(1),
            outcome: AgentOutcome::Failed {
                reason: "Blocked.".into(),
            },
            summary: "s".into(),
        },
        ToolCall::Message {
            to: Recipient::Builder,
            body: "Which?".into(),
            attachments: vec![],
            in_reply_to: None,
        },
        ToolCall::Wait {
            on: WaitOn::Reply {
                message: MessageId::from_u128(1),
                from: Party::Builder,
            },
            deadline: None,
            task: TaskId::from_u128(1),
            progress: progress.clone(),
        },
        ToolCall::Schedule {
            when: Schedule::Once { at: at(5) },
            does: JobRequest::Wake {
                agent: agent(2),
                note: "Check CI.".into(),
            },
        },
        ToolCall::CancelJob {
            job: JobId::from_u128(1),
        },
        ToolCall::RaiseTask {
            title: "Bug".into(),
            ask: "Button overlaps.".into(),
            part: None,
            attachments: vec![FileId::from_u128(2)],
        },
        ToolCall::UpdateTask {
            task: TaskId::from_u128(1),
            progress,
            effort: Some(Effort::Max),
        },
        ToolCall::WriteSpec {
            spec: SpecKind::Add,
            status: SpecStatus::Approved,
            body: "# ADD".into(),
        },
        ToolCall::AddFile {
            name: "shot.png".into(),
            media_type: "image/png".into(),
            size: 10,
            hash: ContentHash("sha256:cd".into()),
        },
        ToolCall::SetPreview {
            url: "http://localhost:5173".into(),
        },
        ToolCall::LoadSkill { skill: skill() },
        ToolCall::StartProcess {
            name: ProcessName("dev".into()),
            command: "npm run dev".into(),
        },
        ToolCall::StopProcess {
            name: ProcessName("dev".into()),
        },
        ToolCall::Remember {
            note: NoteName("deploys".into()),
            body: "Use the staging slot first.".into(),
        },
        ToolCall::FormTeam {
            leads: vec![LeadSpec {
                role: "Product".into(),
                charter: "c".into(),
                effort: range,
                budget: Money::from_dollars(5),
            }],
        },
        ToolCall::SetEffort {
            agent: agent(1),
            range,
        },
        ToolCall::SetBudget {
            lead: agent(1),
            budget: Money::from_dollars(8),
        },
        ToolCall::ReadLog {
            from: Seq(1),
            to: None,
        },
        ToolCall::ReadMemories { agent: agent(1) },
        ToolCall::ProposeChange(Proposal::Trial {
            change: TrialChange::CompactMemory {
                agent: agent(1),
                notes: vec![(NoteName("all".into()), "merged".into())],
            },
            eval: EvalSet {
                items: vec![],
                runs_per_arm: 3,
            },
        }),
        ToolCall::ProposeChange(Proposal::Team {
            changes: vec![
                TeamChange::Fold {
                    lead: agent(1),
                    into: agent(3),
                },
                TeamChange::Promote {
                    worker: agent(2),
                    effort: range,
                    budget: Money::from_dollars(2),
                },
            ],
            telemetry: "Lead 1 idle for 3 tasks.".into(),
            team_toml: "[leads]".into(),
        }),
    ];
    for call in calls {
        let action = Action {
            id: ActionId::from_u128(1),
            agent: agent(1),
            request: Request::Tool {
                driver: DriverId::from_u128(1),
                call,
            },
        };
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(serde_json::from_str::<Action>(&json).unwrap(), action);
    }
}

#[test]
fn inbox_items_and_routines_round_trip() {
    let items = [
        InboxItem::WaitEnded {
            wait: WaitId::from_u128(1),
            end: WaitEnd::CannotHappen,
        },
        InboxItem::NoDriver {
            agent: agent(2),
            why: vec![Incompatible::MachineFull {
                machine: MachineId::from_u128(1),
            }],
        },
        InboxItem::Nudge {
            task: TaskId::from_u128(1),
            turns: 2,
        },
        InboxItem::BudgetReached { lead: agent(1) },
    ];
    for item in items {
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(serde_json::from_str::<InboxItem>(&json).unwrap(), item);
    }
    let routines = [
        ProjectRoutine::WaitDeadline {
            agent: agent(1),
            wait: WaitId::from_u128(1),
        },
        ProjectRoutine::DriverWaitDeadline { agent: agent(1) },
    ];
    for routine in routines.map(ProjectJob::Routine) {
        let json = serde_json::to_string(&routine).unwrap();
        assert_eq!(serde_json::from_str::<ProjectJob>(&json).unwrap(), routine);
    }
}
