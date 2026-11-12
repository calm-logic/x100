//! One example of every method: params and result. They are round-tripped when built, they are
//! what `PROTOCOL.md` shows, and the test that compares the document with the method table starts
//! from them.

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::time::Duration;

use serde_json::Value;
use x100_core::action::{AskMeAbout, Decision};
use x100_core::credential::{CredentialScope, Projects};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::Envelope;
use x100_core::file::{ContentHash, FileRef};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::git::CommitId;
use x100_core::harness::{HarnessModel, ModelName};
use x100_core::id::*;
use x100_core::inbox::InboxItem;
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope, SkillVersion};
use x100_core::money::{Money, Percent};
use x100_core::party::Party;
use x100_core::project::{SettingChange, Settings};
use x100_core::settings::DaemonSetting;
use x100_core::testing::fernleaf;
use x100_core::time::Timestamp;
use x100_core::wait::{Wait, WaitOn};
use x100_proto::*;

pub struct Sample {
    pub name: &'static str,
    pub params: Value,
    pub result: Value,
}

const T0: u64 = 1_794_387_600_000;

pub fn id<T>(make: fn(Ulid) -> T, n: u64) -> T {
    make(Ulid::from_parts(T0, u128::from(n)))
}

pub fn at(secs: u64) -> Timestamp {
    Timestamp::from_millis(T0 + secs * 1000)
}

/// Builds the sample and proves it survives the wire: as a line, as a request, as a response.
fn s<M>(params: M, result: M::Result) -> Sample
where
    M: Method + Clone + PartialEq + Debug,
    M::Result: Clone + PartialEq + Debug,
    Call: From<M>,
{
    let call = Call::from(params.clone());
    assert_eq!(call.method(), M::NAME);
    let request = Message::Request(Request {
        id: Id::Number(7),
        call: call.clone(),
    });
    let line = request.to_line();
    assert!(!line.contains('\n'), "{} is not one line", M::NAME);
    assert_eq!(Message::parse(&line), Ok(request), "{} params", M::NAME);

    let response = Response::ok::<M>(Id::Number(7), &result);
    let line = Message::Response(response.clone()).to_line();
    assert!(!line.contains('\n'));
    let Ok(Message::Response(back)) = Message::parse(&line) else {
        panic!("{} response did not parse", M::NAME);
    };
    assert_eq!(back, response);
    assert_eq!(
        back.into_result::<M>().unwrap(),
        result,
        "{} result",
        M::NAME
    );

    Sample {
        name: M::NAME,
        params: call.params(),
        result: serde_json::to_value(&result).unwrap(),
    }
}

pub fn scope() -> CredentialScope {
    CredentialScope {
        hosts: BTreeSet::from(["api.stripe.com".to_owned()]),
        commands: BTreeSet::from(["stripe".to_owned()]),
        projects: Projects::Only(BTreeSet::from([id(ProjectId::new, 1)])),
    }
}

pub fn file_ref() -> FileRef {
    FileRef {
        id: id(FileId::new, 3),
        name: "sketch.png".into(),
        media_type: "image/png".into(),
        size: 48_213,
        hash: ContentHash(
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
        ),
        added_by: Party::Builder,
        attached_to: None,
    }
}

pub fn settings() -> Settings {
    Settings {
        ask_me_about: AskMeAbout::HighRiskOnly,
        budget: Money::from_dollars(50),
        lab_share: Percent::new(10).unwrap(),
        allowed: BTreeSet::from([HarnessModel {
            harness: id(HarnessId::new, 2),
            model: ModelName("claude-sonnet".into()),
        }]),
    }
}

/// A project state with only its creation, small enough to read.
pub fn small_project() -> ProjectState {
    ProjectState::fold(&fernleaf().log[..1]).unwrap()
}

pub fn project_events(n: usize) -> Vec<Envelope<x100_core::event::ProjectEvent>> {
    fernleaf().log[..n].to_vec()
}

pub fn samples() -> Vec<Sample> {
    let prj = id(ProjectId::new, 1);
    let agt = id(AgentId::new, 2);
    let drv = id(DriverId::new, 4);
    let tsk = id(TaskId::new, 5);
    let msg = id(MessageId::new, 6);
    let fil = id(FileId::new, 3);
    let reference = id(CredentialRef::new, 8);
    let low_high = EffortRange::new(Effort::Low, Effort::High).unwrap();
    let job = id(JobId::new, 9);
    let action = id(ActionId::new, 10);
    let wait = Wait {
        id: id(WaitId::new, 11),
        on: WaitOn::Report(tsk),
        deadline: at(86_400),
        deadline_job: job,
    };

    vec![
        s(
            Hello {
                client: "x100-app".into(),
                protocol: 1,
            },
            Welcome {
                protocol: 1,
                daemon: "0.1.0".into(),
            },
        ),
        s(
            Subscribe { resume: None },
            Subscribed {
                subscription: SubscriptionId(1),
                snapshot: Some(Snapshot {
                    daemon: DaemonState::default(),
                    projects: [(prj, small_project())].into(),
                }),
            },
        ),
        s(
            Unsubscribe {
                subscription: SubscriptionId(1),
            },
            Done {},
        ),
        s(
            ReadLog {
                log: LogId::Project(prj),
                page: Page::NewestFirst {
                    before: None,
                    limit: 2,
                },
            },
            LogPage::Project {
                events: project_events(2),
            },
        ),
        s(
            ReadTranscript {
                project: prj,
                agent: agt,
                after: Some(Position(40)),
                limit: 100,
            },
            TranscriptPage {
                entries: vec![PositionedEntry {
                    position: Position(41),
                    entry: TranscriptEntry {
                        at: at(30),
                        driver: drv,
                        item: TranscriptItem::Text {
                            text: "Reading the migration.".into(),
                        },
                    },
                }],
            },
        ),
        s(
            SubscribeTranscript {
                project: prj,
                agent: agt,
                after: None,
            },
            TranscriptSubscribed {
                subscription: SubscriptionId(2),
            },
        ),
        s(GetDaemon {}, DaemonState::default()),
        s(
            SetDaemonSetting {
                setting: DaemonSetting::StallTimeout(Duration::from_secs(600)),
            },
            Done {},
        ),
        s(
            SetMachineCap {
                machine: id(MachineId::new, 12),
                cap: 8,
            },
            Done {},
        ),
        s(
            RescanHarnesses {},
            Rescanned {
                harnesses: vec![id(HarnessId::new, 2), id(HarnessId::new, 13)],
            },
        ),
        s(
            AddCredential {
                name: "Stripe test key".into(),
                value: Secret::new("sk_test_not_a_real_key"),
                scope: scope(),
            },
            CredentialAdded {
                credential: reference,
            },
        ),
        s(
            ReplaceCredential {
                credential: reference,
                value: Secret::new("sk_test_another_not_real"),
            },
            Done {},
        ),
        s(
            ScopeCredential {
                credential: reference,
                scope: scope(),
            },
            Done {},
        ),
        s(
            RemoveCredential {
                credential: reference,
            },
            Done {},
        ),
        s(
            ListProjects {},
            ProjectList {
                projects: vec![ProjectSummary {
                    project: prj,
                    name: "Fernleaf".into(),
                    path: "/home/ada/fernleaf".into(),
                    live: true,
                    agents: 4,
                }],
            },
        ),
        s(GetProject { project: prj }, small_project()),
        s(
            CreateProject {
                path: "/home/ada/fernleaf".into(),
                name: Some("Fernleaf".into()),
                settings: Some(settings()),
            },
            ProjectCreated { project: prj },
        ),
        s(
            ChangeProjectSetting {
                project: prj,
                change: SettingChange::Budget(Money::from_dollars(80)),
            },
            Done {},
        ),
        s(Reflect { project: prj }, Reflecting { task: tsk }),
        s(
            SendBrief {
                project: prj,
                body: "A landing page for a plant shop, with a waitlist.".into(),
                attachments: vec![fil],
            },
            BriefSent {
                message: msg,
                task: tsk,
            },
        ),
        s(
            SendChat {
                project: prj,
                body: "Use the green from the sketch.".into(),
                attachments: vec![],
                in_reply_to: Some(msg),
            },
            ChatSent { message: msg },
        ),
        s(
            RaiseTask {
                project: prj,
                title: "Footer links are broken".into(),
                ask: "The footer links 404 on mobile.".into(),
                part: Some("Website".into()),
                attachments: vec![fil],
            },
            TaskRaised { task: tsk },
        ),
        s(
            UploadFile {
                project: prj,
                name: "sketch.png".into(),
                media_type: "image/png".into(),
                data: Bytes(b"\x89PNG not really".to_vec()),
            },
            FileUploaded { file: file_ref() },
        ),
        s(
            ReadFile {
                project: prj,
                file: fil,
            },
            FileContent {
                file: file_ref(),
                data: Bytes(b"\x89PNG not really".to_vec()),
            },
        ),
        s(
            RemoveFile {
                project: prj,
                file: fil,
            },
            Done {},
        ),
        s(CancelJob { project: prj, job }, Done {}),
        s(
            DecideAction {
                project: prj,
                action,
                decision: Decision::Approved,
            },
            Done {},
        ),
        s(
            SetLeadBudget {
                project: prj,
                lead: agt,
                budget: Money::from_dollars(15),
            },
            Done {},
        ),
        s(
            SetEffortRange {
                project: prj,
                agent: agt,
                range: low_high,
            },
            Done {},
        ),
        s(
            ReplaceDriver {
                project: prj,
                agent: agt,
            },
            Done {},
        ),
        s(
            ReadInbox {
                project: prj,
                agent: agt,
            },
            Inbox {
                items: vec![InboxEntry {
                    id: id(InboxItemId::new, 14),
                    item: InboxItem::Task { task: tsk },
                    being_handled: false,
                }],
            },
        ),
        s(
            ReadWaits {
                project: prj,
                agent: agt,
            },
            Waits { waits: vec![wait] },
        ),
        s(
            StopDriver {
                project: prj,
                driver: drv,
            },
            Done {},
        ),
        s(
            ReadStartPack {
                project: prj,
                driver: drv,
            },
            StartPack {
                text: "# Charter\n...".into(),
            },
        ),
        s(
            ListMemories {
                project: prj,
                agent: agt,
            },
            Memories {
                notes: vec![MemoryNote {
                    name: NoteName("deploys.md".into()),
                    commit: CommitId("4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()),
                    body: "Deploys go through `make ship`.".into(),
                }],
            },
        ),
        s(
            ListSkills {
                project: prj,
                scope: Some(SkillScope::Shared),
            },
            Skills {
                skills: vec![SkillInfo {
                    skill: SkillRef {
                        scope: SkillScope::Shared,
                        name: SkillName("deploy".into()),
                    },
                    version: SkillVersion(3),
                    body: "---\nname: deploy\nversion: 3\n---\n".into(),
                }],
            },
        ),
        s(ListTrials { project: prj }, Trials { trials: vec![] }),
    ]
}
