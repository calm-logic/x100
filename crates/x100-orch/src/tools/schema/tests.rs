//! Every tool's schema accepts the arguments serde writes for its call and refuses wrong ones, for
//! a sample of every variant of every argument type. The samples come from a match on [`Tool`],
//! and [`Tool::of`] matches on [`ToolCall`], so a new tool cannot be added without a schema, a
//! description and a sample here.

use std::time::Duration;

use serde_json::{Value, json};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::Seq;
use x100_core::file::ContentHash;
use x100_core::git::CommitId;
use x100_core::id::{ActionId, AgentId, FileId, JobId, MessageId, TaskId};
use x100_core::job::{Interval, Schedule};
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
use x100_core::trial::{Check, EvalItem, EvalSet, TrialChange};
use x100_core::wait::WaitOn;

use super::schema;
use crate::tools::Tool;

fn agent(n: u128) -> AgentId {
    AgentId::from_u128(n)
}

fn progress(done: Option<u8>) -> Progress {
    Progress {
        note: "Half done.".into(),
        done: done.map(|d| Percent::new(d).unwrap()),
    }
}

fn lead() -> LeadSpec {
    LeadSpec {
        role: "Tech".into(),
        charter: "Builds it.".into(),
        effort: EffortRange::new(Effort::Low, Effort::High).unwrap(),
        budget: Money::from_dollars(5),
    }
}

fn skill(scope: SkillScope) -> SkillRef {
    SkillRef {
        scope,
        name: SkillName("deploy".into()),
    }
}

/// Calls of `tool` that between them use every variant of every argument type it takes, and
/// both a set and a missing value of every optional argument.
fn samples(tool: Tool) -> Vec<ToolCall> {
    let task = TaskId::from_u128(7);
    match tool {
        Tool::Delegate => vec![
            ToolCall::Delegate {
                to: Delegatee::Child(agent(2)),
                task: Delegated::New {
                    title: "Build it".into(),
                    ask: "All of it.".into(),
                    attachments: vec![FileId::from_u128(3)],
                },
                effort: Effort::Medium,
            },
            ToolCall::Delegate {
                to: Delegatee::NewWorker {
                    role: "Tester".into(),
                    charter: "Tests it.".into(),
                    effort: Some(EffortRange::FULL),
                },
                task: Delegated::Raised(task),
                effort: Effort::Max,
            },
            ToolCall::Delegate {
                to: Delegatee::NewWorker {
                    role: "Tester".into(),
                    charter: "Tests it.".into(),
                    effort: None,
                },
                task: Delegated::Raised(task),
                effort: Effort::Low,
            },
        ],
        Tool::Review => vec![
            ToolCall::Review {
                task,
                verdict: Verdict::Accept,
            },
            ToolCall::Review {
                task,
                verdict: Verdict::SendBack {
                    note: "Tests fail.".into(),
                    effort: Some(Effort::High),
                },
            },
            ToolCall::Review {
                task,
                verdict: Verdict::SendBack {
                    note: "Tests fail.".into(),
                    effort: None,
                },
            },
        ],
        Tool::Report => vec![
            ToolCall::Report {
                task,
                outcome: AgentOutcome::Completed,
                summary: "Done.".into(),
            },
            ToolCall::Report {
                task,
                outcome: AgentOutcome::Failed {
                    reason: "No access.".into(),
                },
                summary: "Stuck.".into(),
            },
        ],
        Tool::Message => vec![
            ToolCall::Message {
                to: Recipient::Agent(agent(2)),
                body: "Hello.".into(),
                attachments: vec![],
                in_reply_to: Some(MessageId::from_u128(4)),
            },
            ToolCall::Message {
                to: Recipient::Builder,
                body: "Hello.".into(),
                attachments: vec![FileId::from_u128(3)],
                in_reply_to: None,
            },
        ],
        Tool::Wait => vec![
            ToolCall::Wait {
                on: WaitOn::Job(JobId::from_u128(5)),
                deadline: Some(Timestamp::from_millis(1_794_387_600_000)),
                task,
                progress: progress(Some(40)),
            },
            ToolCall::Wait {
                on: WaitOn::Report(task),
                deadline: None,
                task,
                progress: progress(None),
            },
            ToolCall::Wait {
                on: WaitOn::Reply {
                    message: MessageId::from_u128(4),
                    from: Party::Builder,
                },
                deadline: None,
                task,
                progress: progress(Some(0)),
            },
            ToolCall::Wait {
                on: WaitOn::Reply {
                    message: MessageId::from_u128(4),
                    from: Party::Agent(agent(2)),
                },
                deadline: None,
                task,
                progress: progress(Some(100)),
            },
            ToolCall::Wait {
                on: WaitOn::Decision(ActionId::from_u128(6)),
                deadline: None,
                task,
                progress: progress(None),
            },
        ],
        Tool::Schedule => vec![
            ToolCall::Schedule {
                when: Schedule::Once {
                    at: Timestamp::from_millis(1),
                },
                does: JobRequest::Wake {
                    agent: agent(2),
                    note: "Look again.".into(),
                },
            },
            ToolCall::Schedule {
                when: Schedule::Every {
                    first: Timestamp::from_millis(1),
                    every: Interval::new(Duration::from_millis(3_600_500)).unwrap(),
                },
                does: JobRequest::Delegate {
                    to: agent(2),
                    title: "Smoke tests".into(),
                    ask: "Run them.".into(),
                    effort: Effort::Low,
                },
            },
        ],
        Tool::CancelJob => vec![ToolCall::CancelJob {
            job: JobId::from_u128(5),
        }],
        Tool::RaiseTask => vec![
            ToolCall::RaiseTask {
                title: "Dark mode".into(),
                ask: "Add it.".into(),
                part: Some("Site".into()),
                attachments: vec![],
            },
            ToolCall::RaiseTask {
                title: "Dark mode".into(),
                ask: "Add it.".into(),
                part: None,
                attachments: vec![FileId::from_u128(3)],
            },
        ],
        Tool::UpdateTask => vec![
            ToolCall::UpdateTask {
                task,
                progress: progress(Some(60)),
                effort: Some(Effort::High),
            },
            ToolCall::UpdateTask {
                task,
                progress: progress(None),
                effort: None,
            },
        ],
        Tool::WriteSpec => [SpecKind::Brd, SpecKind::Prd, SpecKind::Add]
            .into_iter()
            .zip([SpecStatus::Draft, SpecStatus::Approved, SpecStatus::Draft])
            .map(|(spec, status)| ToolCall::WriteSpec {
                spec,
                status,
                body: "# Spec".into(),
            })
            .collect(),
        Tool::AddFile => vec![ToolCall::AddFile {
            name: "logo.png".into(),
            media_type: "image/png".into(),
            size: 2048,
            hash: ContentHash("ab12".into()),
        }],
        Tool::SetPreview => vec![ToolCall::SetPreview {
            url: "http://127.0.0.1:5173".into(),
        }],
        Tool::SetUpProject => vec![
            ToolCall::SetUpProject {
                name: "Todo app".into(),
                intent: "A todo list in one page.".into(),
                brief: Some("One index.html with a list that survives a reload.".into()),
            },
            ToolCall::SetUpProject {
                name: "Todo".into(),
                intent: "A todo list in one page.".into(),
                brief: None,
            },
        ],
        Tool::ListIntegrations => vec![ToolCall::ListIntegrations {}],
        Tool::LinkProject => vec![
            ToolCall::LinkProject {
                homebox_project: "foobar".into(),
                environment: None,
            },
            ToolCall::LinkProject {
                homebox_project: "builder/foobar".into(),
                environment: Some("dev".into()),
            },
        ],
        Tool::Deploy => vec![
            ToolCall::Deploy { environment: None },
            ToolCall::Deploy {
                environment: Some("production".into()),
            },
        ],
        Tool::LoadSkill => vec![
            ToolCall::LoadSkill {
                skill: skill(SkillScope::Shared),
            },
            ToolCall::LoadSkill {
                skill: skill(SkillScope::Agent(agent(2))),
            },
        ],
        Tool::StartProcess => vec![ToolCall::StartProcess {
            name: ProcessName("dev".into()),
            command: "npm run dev".into(),
        }],
        Tool::StopProcess => vec![ToolCall::StopProcess {
            name: ProcessName("dev".into()),
        }],
        Tool::Remember => vec![ToolCall::Remember {
            note: NoteName("stack".into()),
            body: "We use Vite.".into(),
            sources: vec![Seq(3), Seq(9)],
        }],
        Tool::ReadNote => vec![ToolCall::ReadNote {
            note: NoteName("stack".into()),
        }],
        Tool::SearchNotes => vec![ToolCall::SearchNotes {
            query: "vite".into(),
        }],
        Tool::SearchLog => vec![
            ToolCall::SearchLog {
                query: "vite".into(),
                limit: Some(5),
            },
            ToolCall::SearchLog {
                query: "vite".into(),
                limit: None,
            },
        ],
        Tool::FormTeam => vec![ToolCall::FormTeam {
            leads: vec![lead(), lead()],
        }],
        Tool::SetEffort => vec![ToolCall::SetEffort {
            agent: agent(2),
            range: EffortRange::FULL,
        }],
        Tool::SetBudget => vec![ToolCall::SetBudget {
            lead: agent(2),
            budget: Money::from_micros(1_500_000),
        }],
        Tool::ReadLog => vec![
            ToolCall::ReadLog {
                from: Seq(1),
                to: Some(Seq(40)),
            },
            ToolCall::ReadLog {
                from: Seq(1),
                to: None,
            },
        ],
        Tool::ReadMemories => vec![ToolCall::ReadMemories { agent: agent(2) }],
        Tool::ProposeChange => vec![
            ToolCall::ProposeChange(Proposal::Trial {
                change: TrialChange::EditSkill {
                    skill: skill(SkillScope::Shared),
                    body: "Steps.".into(),
                },
                eval: EvalSet {
                    items: vec![
                        EvalItem::PastTask {
                            task,
                            checkpoint: CommitId("0123abcd".into()),
                        },
                        EvalItem::Check(Check::Script {
                            command: "cargo test".into(),
                        }),
                        EvalItem::Check(Check::Rubric {
                            text: "Clear.".into(),
                        }),
                        EvalItem::Probe {
                            question: "Which port?".into(),
                            answer: "5173".into(),
                        },
                    ],
                    runs_per_arm: 3,
                },
            }),
            ToolCall::ProposeChange(Proposal::Trial {
                change: TrialChange::NewSkill {
                    skill: skill(SkillScope::Agent(agent(9))),
                    body: "Steps.".into(),
                },
                eval: EvalSet {
                    items: vec![],
                    runs_per_arm: 1,
                },
            }),
            ToolCall::ProposeChange(Proposal::Trial {
                change: TrialChange::CompactMemory {
                    agent: agent(2),
                    notes: vec![(NoteName("stack".into()), "Vite.".into())],
                },
                eval: EvalSet {
                    items: vec![],
                    runs_per_arm: 1,
                },
            }),
            ToolCall::ProposeChange(Proposal::Team {
                changes: vec![
                    TeamChange::AddLead(lead()),
                    TeamChange::Fold {
                        lead: agent(2),
                        into: agent(3),
                    },
                    TeamChange::Promote {
                        worker: agent(4),
                        effort: EffortRange::FULL,
                        budget: Money::from_dollars(2),
                    },
                    TeamChange::Retire { agent: agent(5) },
                ],
                telemetry: "Tech is idle.".into(),
                team_toml: "[team]".into(),
            }),
        ],
    }
}

/// The arguments a client sends for `call`: its variant's serde form, without the name.
fn arguments(call: &ToolCall) -> Value {
    match serde_json::to_value(call).unwrap() {
        Value::Object(m) => m.into_iter().next().unwrap().1,
        other => panic!("a call serialises as an object: {other}"),
    }
}

fn validator(tool: Tool) -> jsonschema::Validator {
    let schema = schema(tool);
    assert_eq!(
        schema["type"],
        "object",
        "{}: MCP needs an object",
        tool.name()
    );
    jsonschema::draft202012::new(&schema)
        .unwrap_or_else(|e| panic!("{}'s schema is not valid JSON Schema: {e}", tool.name()))
}

#[test]
fn every_tool_has_a_name_a_description_and_a_schema_that_fits_serde() {
    for tool in Tool::ALL {
        assert_eq!(Tool::named(tool.name()), Some(tool));
        assert!(
            tool.description().len() > 30,
            "{} says too little",
            tool.name()
        );
        let valid = validator(tool);
        for call in samples(tool) {
            assert_eq!(Tool::of(&call), tool);
            let args = arguments(&call);
            let errors: Vec<String> = valid.iter_errors(&args).map(|e| e.to_string()).collect();
            assert!(
                errors.is_empty(),
                "{} refuses {args}: {errors:?}",
                tool.name()
            );
            assert_eq!(tool.parse(args).as_ref(), Ok(&call));
        }
    }
}

#[test]
fn every_tool_refuses_arguments_of_the_wrong_shape_as_serde_does() {
    for tool in Tool::ALL {
        let valid = validator(tool);
        let mut wrong = vec![json!([]), json!("text")];
        // An empty object is wrong only for a tool that requires something.
        if schema(tool)["required"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
        {
            wrong.push(json!({}));
        }
        for call in samples(tool) {
            let args = arguments(&call);
            // Each argument in turn replaced by a value of a type none of them takes.
            for key in args.as_object().unwrap().keys() {
                let mut bad = args.clone();
                bad[key] = json!(true);
                wrong.push(bad);
            }
            let mut extra = args.clone();
            extra["surprise"] = json!(1);
            assert!(
                !valid.is_valid(&extra),
                "{} takes an unknown field",
                tool.name()
            );
        }
        for bad in wrong {
            assert!(!valid.is_valid(&bad), "{} accepts {bad}", tool.name());
            assert!(tool.parse(bad.clone()).is_err(), "serde accepts {bad}");
        }
    }
}

#[test]
fn an_id_of_the_wrong_kind_is_refused() {
    let valid = validator(Tool::CancelJob);
    let job = json!({ "job": JobId::from_u128(5) });
    assert!(valid.is_valid(&job));
    let task = json!({ "job": TaskId::from_u128(5) });
    assert!(!valid.is_valid(&task));
    assert!(Tool::CancelJob.parse(task).is_err());
}
