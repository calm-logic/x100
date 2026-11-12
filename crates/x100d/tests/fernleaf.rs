//! The Fernleaf script, the one the orchestrator's own test plays in its process, played through a
//! real daemon: the daemon is a child process, the client library is the only way in, and the
//! script file is the orchestrator's, read from where it lives.
//!
//! The script names times on a fake clock, and its turns follow one another in one order. So the
//! daemon runs with `--fake-clock`, which gives it a clock that moves only when this test says,
//! through `daemon.set_clock`, and one turn at a time.

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use common::*;
use x100_client::Client;
use x100_core::action::AskMeAbout;
use x100_core::agent::Position;
use x100_core::driver::{DriverState, EndReason, RetireReason};
use x100_core::fold::{ChatItem, ProjectState};
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::id::{AgentId, ProjectId, TaskId};
use x100_core::job::{JobState, ProjectJob};
use x100_core::message::Route;
use x100_core::money::{Money, Percent};
use x100_core::party::Party;
use x100_core::project::Settings;
use x100_core::settings::DaemonSetting;
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::task::{Outcome, TaskState};
use x100_core::time::Timestamp;
use x100_proto::{
    CancelJob, CreateProject, ErrorCode, GetProject, ReadStartPack, SendChat, SetClock,
    SetDaemonSetting,
};

const SCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../x100-orch/tests/scripts");
const START: Timestamp = Timestamp::from_millis(1_794_387_600_000);
const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);
/// The whole run takes a few seconds; this only bounds a hang.
const RUN: Duration = Duration::from_secs(120);

struct Run {
    env: Env,
    daemon: DaemonProc,
    client: Client,
    project: ProjectId,
}

impl Run {
    async fn start() -> Run {
        let env = Env::new();
        let daemon = DaemonProc::spawn(vec![
            "--data-dir".into(),
            env.data.clone().into(),
            "--projects-root".into(),
            env.projects_root().into(),
            "--scripts-dir".into(),
            SCRIPTS.into(),
            "--fake-harness".into(),
            env!("CARGO_BIN_EXE_x100d-fake-harness").into(),
            "--fake-clock".into(),
            START.as_millis().to_string().into(),
        ]);
        let client = env.client().await;
        // Long enough that moving the clock a day on expires no wait the script did not mean to.
        client
            .request(SetDaemonSetting {
                setting: DaemonSetting::DefaultWaitDeadline(7 * DAY),
            })
            .await
            .expect("the setting is taken");
        // Every agent on the model whose warm period an hour outlasts, so the agent whose job
        // wakes it after an hour starts again from its start pack, as in the orchestrator's run.
        let harness = client.with_state(|s| {
            s.daemon
                .harnesses()
                .find(|h| {
                    h.kind
                        == HarnessKind::Custom {
                            name: "fake-fernleaf".into(),
                        }
                })
                .map(|h| h.id)
                .expect("the script's harness")
        });
        let settings = Settings {
            ask_me_about: AskMeAbout::HighRiskOnly,
            budget: Money::from_dollars(50),
            lab_share: Percent::new(10).unwrap(),
            allowed: [HarnessModel {
                harness,
                model: ModelName("fake-brief".into()),
            }]
            .into_iter()
            .collect(),
        };
        let project = client
            .request(CreateProject {
                dir: None,
                settings: Some(settings),
            })
            .await
            .expect("the project is created")
            .project;
        let run = Run {
            env,
            daemon,
            client,
            project,
        };
        run.settle().await;
        run
    }

    fn state(&self) -> ProjectState {
        self.client
            .with_state(|s| s.projects.get(&self.project).cloned())
            .expect("the client holds the project")
    }

    /// Waits until the client holds everything the daemon has written so far, and nothing more
    /// happens without the builder or the clock. A request answers only after its events are in
    /// the log, so the log's end read now is a point the settled state must have reached.
    async fn settle(&self) {
        let after = self
            .client
            .request(GetProject {
                project: self.project,
            })
            .await
            .expect("the project")
            .seq();
        let project = self.project;
        let settled = self
            .client
            .wait_for(RUN, |s| {
                let p = s.projects.get(&project)?;
                (p.seq() >= after && quiet(p)).then_some(())
            })
            .await;
        if settled.is_err() {
            panic!("the run did not settle\n{}", summary(&self.state()));
        }
    }

    async fn clock_to(&self, at: Timestamp) {
        self.client
            .request(SetClock { at })
            .await
            .expect("the clock moves");
        self.settle().await;
    }
}

/// No turn running or starting, and nothing in any inbox waiting for one.
fn quiet(s: &ProjectState) -> bool {
    let busy = s.drivers().any(|d| {
        matches!(
            d.state,
            DriverState::Starting | DriverState::Running | DriverState::Compacting
        )
    });
    !busy && s.agents().all(|a| a.inbox.waiting().next().is_none())
}

fn summary(s: &ProjectState) -> String {
    let mut out = String::new();
    for a in s.agents() {
        out.push_str(&format!(
            "{} {} waits={} tasks={}\n",
            a.role,
            a.state.name(),
            a.waits.len(),
            a.open_tasks.len()
        ));
    }
    let failed: Vec<_> = s.actions().filter_map(|a| a.error.clone()).collect();
    out.push_str(&format!("failed calls: {failed:#?}\n"));
    out
}

fn agent(s: &ProjectState, role: &str) -> AgentId {
    s.agents()
        .find(|a| a.role == role)
        .map(|a| a.id)
        .unwrap_or_else(|| panic!("no agent {role}"))
}

fn task(s: &ProjectState, title: &str) -> TaskId {
    s.tasks()
        .find(|t| t.task.title == title)
        .map(|t| t.task.id)
        .unwrap_or_else(|| panic!("no task {title}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn fernleaf_runs_through_the_daemon() {
    let mut run = Run::start().await;
    let project = run.project;
    run.client
        .send_chat(
            project,
            "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
            vec![],
            None,
        )
        .await
        .expect("the first message is sent");
    run.settle().await;

    // The CEO kept working while its question was open: it accepted the site meanwhile.
    let s = run.state();
    let question = s
        .open_questions()
        .next()
        .expect("the CEO asked the builder")
        .message
        .id;
    assert!(matches!(
        s.task(task(&s, "Build the site")).unwrap().state,
        TaskState::Done(..)
    ));
    run.client
        .request(SendChat {
            project,
            body: "Forest greens.".into(),
            attachments: vec![],
            in_reply_to: Some(question),
        })
        .await
        .expect("the answer is sent");
    run.settle().await;

    // An hour on, the research job wakes its agent; then two mornings of smoke tests.
    run.clock_to(START + HOUR).await;
    run.clock_to(START + 2 * HOUR).await;
    run.clock_to(START + 2 * HOUR + DAY).await;
    let smoke = run
        .state()
        .jobs()
        .find(|j| matches!(j.job.does, ProjectJob::Delegate { .. }))
        .map(|j| j.job.id)
        .expect("the daily job");
    run.client
        .request(CancelJob {
            project,
            job: smoke,
        })
        .await
        .expect("the builder cancels it");
    run.settle().await;

    // Time does not run backwards.
    let back = run.client.request(SetClock { at: START }).await;
    assert!(
        matches!(back, Err(x100_client::ClientError::Rpc(ref e)) if e.code == ErrorCode::Rejected),
        "{back:?}"
    );

    let s = run.state();
    let ceo = s.project().unwrap().ceo;
    let role = |r: &str| agent(&s, r);
    let (tech, product, marketing) = (role("Tech"), role("Product"), role("Marketing"));

    // The team tree: three leads, workers under two of them, and a worker with a worker.
    let leads: BTreeSet<_> = s
        .project()
        .unwrap()
        .team
        .members()
        .filter(|(_, p)| matches!(p, Position::Lead { .. }))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(leads, BTreeSet::from([tech, product, marketing]));
    let under = |a: AgentId| -> BTreeSet<String> {
        s.children(a)
            .into_iter()
            .map(|c| s.agent(c).unwrap().role.clone())
            .collect()
    };
    assert_eq!(
        under(tech),
        ["Backend", "Frontend"].map(String::from).into()
    );
    assert_eq!(under(product), ["Research"].map(String::from).into());
    assert_eq!(
        under(role("Backend")),
        ["Migrations"].map(String::from).into()
    );
    assert!(under(marketing).is_empty());

    // The BRD, written by the CEO and approved once the builder answered.
    let brd = s.spec(SpecKind::Brd).expect("a BRD");
    assert_eq!((brd.spec.author, brd.spec.revision), (ceo, 2));
    assert_eq!(brd.spec.status, SpecStatus::Approved);
    assert!(s.spec(SpecKind::Prd).is_some() && s.spec(SpecKind::Add).is_some());

    // The question to the builder, answered.
    let asked = s
        .messages()
        .find(|m| m.message.route == Route::CeoToBuilder && m.awaited)
        .expect("a question to the builder");
    let answer = s.message(asked.reply.expect("answered")).unwrap();
    assert_eq!(answer.message.route, Route::BuilderToCeo);

    // The final report, in the chat, on the brief; every task done.
    let brief = task(&s, "Launch Fernleaf");
    let TaskState::Done(asg, report) = &s.task(brief).unwrap().state else {
        panic!("the brief is not done");
    };
    assert_eq!(
        (asg.delegator, &report.outcome),
        (Party::Builder, &Outcome::Completed)
    );
    assert!(s.chat().iter().any(|c| matches!(
        &c.item,
        ChatItem::Report { task, .. } if *task == brief
    )));
    for t in s.tasks() {
        assert!(
            matches!(t.state, TaskState::Done(..)),
            "{} is {}",
            t.task.title,
            t.state.name()
        );
    }
    let smoke = s.job(smoke).unwrap();
    assert_eq!((smoke.fired, smoke.state), (2, JobState::Cancelled));

    // The CEO set the project up from the first message, in a code directory under the projects
    // root, and what the workers wrote is on its main branch.
    assert_eq!(s.project().unwrap().name(), Some("Fernleaf"));
    let dir = run.env.projects_root().join("fernleaf");
    assert_eq!(
        s.project().unwrap().code_dir.as_deref(),
        Some(dir.as_path())
    );
    for file in [
        "site/index.html",
        "site/mobile.css",
        "api/waitlist.ts",
        "migrations/0001.sql",
        "copy/launch.md",
    ] {
        assert!(Path::new(&dir).join(file).exists(), "{file} is not on main");
    }

    // The worker woken by its job started again from its start pack, as the daemon records it.
    let research = &s.agent(role("Research")).unwrap().drivers;
    assert_eq!(
        s.driver(research[0]).unwrap().state,
        DriverState::Ended(EndReason::Retired(RetireReason::WarmTooLong))
    );
    let pack = run
        .client
        .request(ReadStartPack {
            project,
            driver: research[1],
        })
        .await
        .expect("a start pack")
        .text;
    assert!(pack.contains("Three found; pricing to recheck."), "{pack}");

    run.daemon.terminate();
    let left = run.env.recorded_pids();
    assert!(left.is_empty(), "drivers left: {left:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_on_the_real_clock_refuses_to_move_it() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let moved = client.request(SetClock { at: START + DAY }).await;
    assert!(
        matches!(moved, Err(x100_client::ClientError::Rpc(ref e)) if e.code == ErrorCode::WrongState),
        "{moved:?}"
    );
    daemon.terminate();
}
