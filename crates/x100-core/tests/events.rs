//! Every event of both logs survives the trip to JSON and back.

use std::collections::BTreeSet;
use std::time::Duration;

use x100_core::action::{
    Action, ActionEvent, ActionKind, AskMeAbout, Classification, ClassifiedBy, Decision, Request,
    Risk,
};
use x100_core::agent::{Agent, AgentEvent, Position};
use x100_core::credential::{
    Credential, CredentialScope, CredentialUse, Projects, Refusal, UseOutcome, Via,
};
use x100_core::driver::{
    Driver, DriverEvent, Failure, Handoff, HandoffCause, InFlight, Incompatible, Purpose,
    RetireReason, StartReason, TakeoverCause, Unfit, View,
};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent, Seq};
use x100_core::file::{ContentHash, FileRef};
use x100_core::git::CommitId;
use x100_core::harness::{
    CoolOff, Harness, HarnessKind, HarnessModel, Login, Machine, ModelName, ModelOffer, OneOffCall,
    OneOffPurpose, PassedOver, ProviderFailure, Support,
};
use x100_core::id::*;
use x100_core::inbox::InboxItem;
use x100_core::job::{DaemonJob, Interval, Job, JobEvent, ProjectJob, Schedule};
use x100_core::ledger::{BudgetReached, Charge, ChargeSource, CostBasis, Payer};
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope, SkillVersion};
use x100_core::message::{Message, Route};
use x100_core::money::{LongContext, Money, Percent, Price, Rates, Usage};
use x100_core::party::{Actor, Party};
use x100_core::process::{Process, ProcessEnd, ProcessName};
use x100_core::project::{SettingChange, Settings};
use x100_core::settings::DaemonSetting;
use x100_core::spec::{SpecKind, SpecRevision, SpecStatus};
use x100_core::task::{Assignment, Opening, Outcome, Report, Task, TaskEvent, TaskFailure};
use x100_core::time::Timestamp;
use x100_core::tool::{Delegated, Delegatee, ToolCall};
use x100_core::trial::{Arm, Check, EvalItem, EvalSet, RunScore, Trial, TrialChange, TrialEvent};
use x100_core::wait::{Wait, WaitOn};

fn agent(n: u128) -> AgentId {
    AgentId::from_u128(n)
}

fn hm() -> HarnessModel {
    HarnessModel {
        harness: HarnessId::from_u128(1),
        model: ModelName("model-a".into()),
    }
}

fn skill() -> SkillRef {
    SkillRef {
        scope: SkillScope::Shared,
        name: SkillName("deploy".into()),
    }
}

fn commit() -> CommitId {
    CommitId("0123abcd".into())
}

fn at(ms: u64) -> Timestamp {
    Timestamp::from_millis(ms)
}

fn project_events() -> Vec<ProjectEvent> {
    use ProjectEvent as P;
    let settings = Settings {
        ask_me_about: AskMeAbout::HighRiskOnly,
        budget: Money::from_dollars(50),
        lab_share: Percent::new(10).unwrap(),
        allowed: BTreeSet::from([hm()]),
    };
    let assignment = Assignment {
        delegator: Party::Builder,
        assignee: agent(0),
        effort: Effort::High,
    };
    let wait = Wait {
        id: WaitId::from_u128(1),
        on: WaitOn::Job(JobId::from_u128(1)),
        deadline: at(9),
        deadline_job: JobId::from_u128(2),
    };
    vec![
        P::Created {
            name: "Fernleaf".into(),
            ceo: agent(0),
            charter: "Own the outcome.".into(),
            effort: EffortRange::FULL,
            settings,
        },
        P::SettingChanged(SettingChange::Allow {
            what: hm(),
            allowed: false,
        }),
        P::PreviewSet {
            url: "http://localhost:3000".into(),
        },
        P::AgentAdded(Agent {
            id: agent(1),
            role: "Tech".into(),
            charter: "Build it.".into(),
            position: Position::Lead {
                effort: EffortRange::new(Effort::Medium, Effort::Max).unwrap(),
                budget: Money::from_dollars(10),
            },
        }),
        P::Agent {
            agent: agent(1),
            event: AgentEvent::Delivered {
                item: InboxItemId::from_u128(1),
                what: InboxItem::Decision {
                    action: ActionId::from_u128(1),
                    decision: Decision::Approved,
                },
            },
        },
        P::Agent {
            agent: agent(1),
            event: AgentEvent::WaitOpened(wait),
        },
        P::Agent {
            agent: agent(1),
            event: AgentEvent::NoDriver(vec![
                Incompatible::CoolingOff(hm()),
                Incompatible::LoginFull {
                    login: Login("/home/b/.claude".into()),
                },
                Incompatible::Budget(BudgetReached::Lead(agent(1))),
            ]),
        },
        P::DriverStarted(Driver {
            id: DriverId::from_u128(2),
            agent: agent(1),
            harness: HarnessId::from_u128(1),
            model: ModelName("model-a".into()),
            effort: Effort::High,
            machine: MachineId::from_u128(1),
            purpose: Purpose::Work,
            view: View::Terminal,
            why: StartReason::Handoff(Handoff {
                replaces: DriverId::from_u128(1),
                cause: HandoffCause::Takeover(TakeoverCause::Failed(Failure::Stalled {
                    silent_for: Duration::from_secs(300),
                })),
                in_flight: InFlight::Command {
                    command: "cargo test".into(),
                },
            }),
        }),
        P::Driver {
            driver: DriverId::from_u128(2),
            event: DriverEvent::TurnStarted {
                given: vec![InboxItemId::from_u128(1)],
            },
        },
        P::Driver {
            driver: DriverId::from_u128(2),
            event: DriverEvent::Retired(RetireReason::Unfit(Unfit::CoolingOff)),
        },
        P::ProcessStarted(Process {
            agent: agent(1),
            name: ProcessName("dev".into()),
            command: "npm run dev".into(),
        }),
        P::ProcessStopped {
            agent: agent(1),
            name: ProcessName("dev".into()),
            end: ProcessEnd::Exited { code: Some(1) },
        },
        P::BranchRefreshed {
            agent: agent(1),
            from: Party::Agent(agent(0)),
            commit: commit(),
        },
        P::Checkpointed {
            agent: agent(1),
            commit: commit(),
        },
        P::OneOffCalled(OneOffCall {
            id: OneOffCallId::from_u128(1),
            purpose: OneOffPurpose::Classify {
                action: ActionId::from_u128(1),
            },
            harness: HarnessId::from_u128(1),
            model: ModelName("model-a".into()),
        }),
        P::Charged(Charge {
            payer: Payer::Lead(agent(1)),
            source: ChargeSource::Turn {
                driver: DriverId::from_u128(2),
                agent: agent(1),
            },
            usage: Usage {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_write_short: 5,
                cache_write_long: 0,
            },
            cost: Money::from_micros(400),
            basis: CostBasis::Listed,
        }),
        P::BudgetReached(BudgetReached::Project),
        P::TaskOpened {
            task: Task {
                id: TaskId::from_u128(1),
                title: "Build the landing page".into(),
                ask: "As in the brief.".into(),
                part: Some("Website".into()),
                raised_by: Party::Builder,
                attachments: vec![FileId::from_u128(1)],
            },
            opening: Opening::Delegated(assignment),
        },
        P::Task {
            task: TaskId::from_u128(1),
            event: TaskEvent::Reported(Report {
                outcome: Outcome::Failed(TaskFailure::DriverFailures(vec![
                    Failure::ProcessExited { code: Some(137) },
                ])),
                summary: "Three drivers failed.".into(),
            }),
        },
        P::MessageSent(Message {
            id: MessageId::from_u128(1),
            route: Route::CeoToBuilder,
            body: "Which colour?".into(),
            attachments: vec![],
            in_reply_to: None,
        }),
        P::JobScheduled(Job {
            id: JobId::from_u128(1),
            when: Schedule::Every {
                first: at(0),
                every: Interval::new(Duration::from_secs(86_400)).unwrap(),
            },
            does: ProjectJob::Delegate {
                scheduled_by: agent(0),
                to: agent(1),
                title: "Smoke tests".into(),
                ask: "Run them.".into(),
                effort: Effort::Low,
            },
        }),
        P::Job {
            job: JobId::from_u128(1),
            event: JobEvent::Cancelled { by: Actor::Builder },
        },
        P::ActionProposed(Action {
            id: ActionId::from_u128(1),
            agent: agent(1),
            request: Request::Tool {
                driver: DriverId::from_u128(2),
                call: ToolCall::Delegate {
                    to: Delegatee::NewWorker {
                        role: "QA".into(),
                        charter: "Test.".into(),
                        effort: None,
                    },
                    task: Delegated::New {
                        title: "Test".into(),
                        ask: "Test it.".into(),
                        attachments: vec![],
                    },
                    effort: Effort::Medium,
                },
            },
        }),
        P::Action {
            action: ActionId::from_u128(1),
            event: ActionEvent::Classified(Classification {
                kind: ActionKind::Other,
                risk: Risk::Low,
                by: ClassifiedBy::Classifier(OneOffCallId::from_u128(1)),
                why: "Routine delegation.".into(),
            }),
        },
        P::SpecWritten(SpecRevision {
            kind: SpecKind::Brd,
            author: agent(0),
            revision: 1,
            status: SpecStatus::Draft,
            body: "# BRD".into(),
        }),
        P::FileAdded(FileRef {
            id: FileId::from_u128(1),
            name: "sketch.png".into(),
            media_type: "image/png".into(),
            size: 1024,
            hash: ContentHash("sha256:ab".into()),
            added_by: Party::Builder,
            attached_to: Some(MessageId::from_u128(1)),
        }),
        P::FileRemoved {
            file: FileId::from_u128(1),
        },
        P::MemoryWritten {
            agent: agent(1),
            note: NoteName("stack".into()),
            commit: commit(),
        },
        P::SkillLoaded {
            agent: agent(1),
            skill: skill(),
            version: SkillVersion(2),
        },
        P::TrialProposed(Trial {
            id: TrialId::from_u128(1),
            change: TrialChange::EditSkill {
                skill: skill(),
                body: "Do it this way.".into(),
            },
            eval: EvalSet {
                items: vec![
                    EvalItem::PastTask {
                        task: TaskId::from_u128(1),
                        checkpoint: commit(),
                    },
                    EvalItem::Check(Check::Rubric {
                        text: "Is it clear?".into(),
                    }),
                    EvalItem::Probe {
                        question: "Which database?".into(),
                        answer: "Postgres".into(),
                    },
                ],
                runs_per_arm: 3,
            },
            branch: "trial/1".into(),
        }),
        P::Trial {
            trial: TrialId::from_u128(1),
            event: TrialEvent::RunFinished(RunScore {
                arm: Arm::Baseline,
                run: 1,
                checks_passed: 2,
                checks_total: 3,
                tool_errors: 1,
                turns: 5,
                duration: Duration::from_secs(40),
                cost: Money::from_micros(900),
                judged: Some(70),
            }),
        },
        P::SkillCommitted {
            skill: skill(),
            version: SkillVersion(3),
            commit: commit(),
            trial: TrialId::from_u128(1),
        },
        P::MemoryCompacted {
            agent: agent(1),
            commit: commit(),
            trial: TrialId::from_u128(2),
        },
        P::CredentialUsed(CredentialUse {
            credential: CredentialRef::from_u128(1),
            agent: agent(1),
            driver: DriverId::from_u128(2),
            via: Via::Proxy {
                host: "api.example.com".into(),
            },
            outcome: UseOutcome::Refused(Refusal::HostNotAllowed),
        }),
    ]
}

fn daemon_events() -> Vec<DaemonEvent> {
    use DaemonEvent as D;
    let rates = Rates {
        input: Money::from_dollars(3),
        output: Money::from_dollars(15),
        cache_read: Money::from_micros(300_000),
        cache_write_short: Money::from_micros(3_750_000),
        cache_write_long: Money::from_dollars(6),
    };
    let price = Price {
        rates,
        long_context: Some(LongContext {
            above: 200_000,
            rates,
        }),
    };
    let harness = Harness {
        id: HarnessId::from_u128(1),
        kind: HarnessKind::Custom {
            name: "local".into(),
        },
        support: Support {
            further_turns: true,
            compaction: false,
            context_fill: true,
            permission_hook: false,
        },
        models: vec![ModelOffer {
            model: ModelName("model-a".into()),
            context_window: 200_000,
            cache_lifetime: Duration::from_secs(300),
            levels: vec![(Effort::Low, price)],
        }],
        login: Some(Login("/home/b/.claude".into())),
    };
    let scope = CredentialScope {
        hosts: BTreeSet::from(["api.example.com".to_owned()]),
        commands: BTreeSet::from(["deploy-tool".to_owned()]),
        projects: Projects::Only(BTreeSet::from([ProjectId::from_u128(1)])),
    };
    vec![
        D::MachineAdded(Machine {
            id: MachineId::from_u128(1),
            name: "laptop".into(),
            harnesses: vec![HarnessId::from_u128(1)],
            cap: 8,
            passed_over: Vec::new(),
        }),
        D::MachineCapSet {
            machine: MachineId::from_u128(1),
            cap: 4,
        },
        D::HarnessAdded(harness.clone()),
        D::HarnessUpdated(harness),
        D::MachineHarnessesSet {
            machine: MachineId::from_u128(1),
            harnesses: vec![HarnessId::from_u128(1)],
            passed_over: vec![PassedOver {
                kind: HarnessKind::CodexTerminal,
                why: "no login at /home/b/.codex".into(),
            }],
        },
        D::CoolOffStarted {
            cool_off: CoolOff {
                of: hm(),
                why: ProviderFailure::RateLimited {
                    retry_after: Some(Duration::from_secs(60)),
                },
                until: at(60_000),
            },
            ends: JobId::from_u128(1),
        },
        D::CoolOffEnded(hm()),
        D::CredentialAdded(Credential {
            reference: CredentialRef::from_u128(1),
            name: "Stripe".into(),
            scope: scope.clone(),
        }),
        D::CredentialReplaced {
            credential: CredentialRef::from_u128(1),
        },
        D::CredentialScopeSet {
            credential: CredentialRef::from_u128(1),
            scope,
        },
        D::CredentialRemoved {
            credential: CredentialRef::from_u128(1),
        },
        D::ProjectAdded {
            project: ProjectId::from_u128(1),
            name: "Fernleaf".into(),
            path: "/home/b/fernleaf".into(),
        },
        D::SettingChanged(DaemonSetting::StallTimeout(Duration::from_secs(600))),
        D::SettingChanged(DaemonSetting::TerminalDriversPerLogin(1)),
        D::JobScheduled(Job {
            id: JobId::from_u128(1),
            when: Schedule::Once { at: at(60_000) },
            does: DaemonJob::EndCoolOff(hm()),
        }),
        D::Job {
            job: JobId::from_u128(1),
            event: JobEvent::Fired { at: at(60_000) },
        },
    ]
}

/// Names each variant, exhaustively, so adding a variant without a sample above fails to compile
/// here.
fn project_variant(e: &ProjectEvent) -> &'static str {
    use ProjectEvent as P;
    match e {
        P::Created { .. } => "Created",
        P::SettingChanged(_) => "SettingChanged",
        P::PreviewSet { .. } => "PreviewSet",
        P::AgentAdded(_) => "AgentAdded",
        P::Agent { .. } => "Agent",
        P::DriverStarted(_) => "DriverStarted",
        P::Driver { .. } => "Driver",
        P::ProcessStarted(_) => "ProcessStarted",
        P::ProcessStopped { .. } => "ProcessStopped",
        P::Checkpointed { .. } => "Checkpointed",
        P::BranchRefreshed { .. } => "BranchRefreshed",
        P::OneOffCalled(_) => "OneOffCalled",
        P::Charged(_) => "Charged",
        P::BudgetReached(_) => "BudgetReached",
        P::TaskOpened { .. } => "TaskOpened",
        P::Task { .. } => "Task",
        P::MessageSent(_) => "MessageSent",
        P::JobScheduled(_) => "JobScheduled",
        P::Job { .. } => "Job",
        P::ActionProposed(_) => "ActionProposed",
        P::Action { .. } => "Action",
        P::SpecWritten(_) => "SpecWritten",
        P::FileAdded(_) => "FileAdded",
        P::FileRemoved { .. } => "FileRemoved",
        P::MemoryWritten { .. } => "MemoryWritten",
        P::SkillLoaded { .. } => "SkillLoaded",
        P::TrialProposed(_) => "TrialProposed",
        P::Trial { .. } => "Trial",
        P::SkillCommitted { .. } => "SkillCommitted",
        P::MemoryCompacted { .. } => "MemoryCompacted",
        P::CredentialUsed(_) => "CredentialUsed",
    }
}

fn daemon_variant(e: &DaemonEvent) -> &'static str {
    use DaemonEvent as D;
    match e {
        D::MachineAdded(_) => "MachineAdded",
        D::MachineCapSet { .. } => "MachineCapSet",
        D::MachineHarnessesSet { .. } => "MachineHarnessesSet",
        D::HarnessAdded(_) => "HarnessAdded",
        D::HarnessUpdated(_) => "HarnessUpdated",
        D::CoolOffStarted { .. } => "CoolOffStarted",
        D::CoolOffEnded(_) => "CoolOffEnded",
        D::CredentialAdded(_) => "CredentialAdded",
        D::CredentialReplaced { .. } => "CredentialReplaced",
        D::CredentialScopeSet { .. } => "CredentialScopeSet",
        D::CredentialRemoved { .. } => "CredentialRemoved",
        D::ProjectAdded { .. } => "ProjectAdded",
        D::SettingChanged(_) => "SettingChanged",
        D::JobScheduled(_) => "JobScheduled",
        D::Job { .. } => "Job",
    }
}

fn round_trip<E>(events: &[E], variant: fn(&E) -> &'static str, expected_variants: usize)
where
    E: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug + Clone,
{
    let names: BTreeSet<_> = events.iter().map(variant).collect();
    assert_eq!(names.len(), expected_variants, "a variant has no sample");
    for (i, event) in events.iter().enumerate() {
        let envelope = Envelope {
            seq: Seq(i as u64 + 1),
            at: at(1_000),
            by: Actor::Daemon,
            event: event.clone(),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let back: Envelope<E> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, envelope, "{json}");
    }
}

#[test]
fn every_project_event_round_trips() {
    round_trip(&project_events(), project_variant, 31);
}

#[test]
fn every_daemon_event_round_trips() {
    round_trip(&daemon_events(), daemon_variant, 15);
}
