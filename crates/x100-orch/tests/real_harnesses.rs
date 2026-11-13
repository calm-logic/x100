//! The real Claude Code and Codex binaries as the CEO's driver, reaching the daemon's tools over
//! MCP, with the model played by the fake provider on localhost. No request leaves the machine:
//! the provider's base URL is the fake one, the key is a dummy, each harness has a clean
//! environment and a throwaway configuration directory, and its proxy is the fake provider,
//! which refuses and records any attempt to reach another host. Each test asserts none happened.
//!
//! A test is skipped, saying so, when its program is not installed.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use x100_core::action::Request;
use x100_core::driver::DriverState;
use x100_core::effort::EffortRange;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessKind, ModelOffer, Support};
use x100_core::harness::{HarnessModel, ModelName};
use x100_core::id::{AgentId, DriverId, HarnessId, ProjectId, TaskId};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::{SettingChange, Settings as ProjectSettings};
use x100_core::settings::DaemonSettings;
use x100_core::spec::SpecKind;
use x100_core::task::{Outcome, TaskState};
use x100_core::time::{Clock, FakeClock};
use x100_core::tool::ToolCall;
use x100_drivers::claude::{ClaudeCode, ClaudeCodeConfig};
use x100_drivers::codex::{Codex, CodexConfig};
use x100_drivers::testing::{FakeProvider, Reply};
use x100_drivers::{DriverHandle, DriverSpec, Harness, StartError};
use x100_jobs::Scheduler;
use x100_orch::{Config, Desk, Orchestrator, ProcessRegistry, ToolServer};
use x100_store::ProjectStore;

const HARNESS: HarnessId = HarnessId::from_u128(11);
const PATIENCE: Duration = Duration::from_secs(120);
const BRD: &str = "# BRD\n\nA one-page site for Fernleaf.\n";

struct Run {
    dir: tempfile::TempDir,
    store: ProjectStore,
    project: ProjectId,
    ceo: AgentId,
    orch: Orchestrator,
    desk: Arc<Desk>,
    _tools: ToolServer,
    provider: FakeProvider,
    /// Allowed only once the model's replies are queued, so the CEO waits for a driver until then.
    model: HarnessModel,
}

/// A harness program on PATH, or `None` with a note so a skipped test is visible.
fn installed(program: &str) -> Option<PathBuf> {
    let found = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file());
    if found.is_none() {
        eprintln!("SKIPPED: {program} is not installed on this machine");
    }
    found
}

/// Gives each driver the variables a test harness needs: a home of its own, the dummy key, and
/// the fake provider as its proxy for everything but loopback.
struct WithEnv {
    inner: Arc<dyn Harness>,
    env: Vec<(String, String)>,
}

#[async_trait::async_trait]
impl Harness for WithEnv {
    fn kind(&self) -> HarnessKind {
        self.inner.kind()
    }

    fn models(&self) -> Vec<ModelOffer> {
        self.inner.models()
    }

    fn supports(&self) -> Support {
        self.inner.supports()
    }

    async fn start(&self, mut spec: DriverSpec) -> Result<DriverHandle, StartError> {
        spec.env.extend(self.env.iter().cloned());
        self.inner.start(spec).await
    }
}

impl Run {
    async fn start(
        model: &str,
        harness: impl FnOnce(&FakeProvider, Arc<dyn Clock>) -> Arc<dyn Harness>,
        key: (&str, &str),
    ) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let clock = FakeClock::new(common::START);
        let shared: Arc<dyn Clock> = Arc::new(clock);
        let store = ProjectStore::open(&dir.path().join("store"), Arc::clone(&shared)).unwrap();
        let code = dir.path().join("fernleaf");
        x100_store::Git::new(code.clone()).prepare().unwrap();
        let ceo = x100_orch::new_id(&*shared, AgentId::new);
        let created = ProjectEvent::Created {
            ceo,
            charter: "Own Fernleaf's launch.".into(),
            effort: EffortRange::FULL,
            settings: ProjectSettings {
                ask_me_about: x100_core::action::AskMeAbout::HighRiskOnly,
                budget: Money::from_dollars(5),
                lab_share: Percent::new(10).unwrap(),
                trial_before_commit: false,
                allowed: Default::default(),
            },
        };
        // Set up already: these tests are about the tools over MCP, not the beginning.
        let identity = x100_core::project::Identity {
            name: "Fernleaf".into(),
            intent: "A waitlist page for a plant shop.".into(),
        };
        store
            .log
            .append_all(vec![
                (Actor::Builder, created),
                (Actor::Builder, ProjectEvent::CodeDirSet { dir: code }),
                (Actor::Agent(ceo), ProjectEvent::SetUp(identity)),
            ])
            .unwrap();

        let provider = FakeProvider::start().await.unwrap();
        let (daemon, daemon_jobs) = common::daemon_log(
            &dir.path().join("daemon"),
            Arc::clone(&shared),
            // In-process, with no sandbox: these tests are about the tools over MCP.
            &DaemonSettings {
                run_without_sandbox: true,
                ..DaemonSettings::default()
            },
        );
        let settings = common::settings_of(&daemon);
        let desk = Desk::new(Arc::clone(&settings), x100_vault::Redactor::for_values(&[]));
        let tools = ToolServer::start(Arc::clone(&desk)).await.unwrap();
        let harness = harness(&provider, Arc::clone(&shared));
        let (processes, _) =
            ProcessRegistry::open_and_reap(dir.path().join("drivers.json")).unwrap();
        let env = vec![
            ("HOME".into(), home.display().to_string()),
            (key.0.into(), key.1.into()),
            ("HTTPS_PROXY".into(), provider.url()),
            ("HTTP_PROXY".into(), provider.url()),
            ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
        ];
        let harness: Arc<dyn Harness> = Arc::new(WithEnv {
            inner: harness,
            env,
        });
        let harnesses = HashMap::from([(HARNESS, harness)]);
        common::add_harnesses(&daemon, &harnesses, &[]);
        let orch = Orchestrator::new(Config {
            clock: Arc::clone(&shared),
            dice: Arc::new(x100_orch::OsDice),
            harnesses,
            machine: common::MACHINE,
            daemon,
            daemon_jobs,
            settings,
            processes: Arc::new(processes),
            desk: Arc::clone(&desk),
            containment: common::uncontained(dir.path()),
            redactor: x100_vault::Redactor::for_values(&[]),
            serial: true,
        });
        let project = ProjectId::from_u128(1);
        orch.attach(
            project,
            store.clone(),
            Scheduler::new(store.log.clone(), shared),
        );
        Run {
            dir,
            store,
            project,
            ceo,
            orch,
            desk,
            _tools: tools,
            provider,
            model: HarnessModel {
                harness: HARNESS,
                model: ModelName(model.into()),
            },
        }
    }

    async fn until(&self, what: &str, done: impl Fn(&ProjectState) -> bool) {
        let (_, mut events) = self.store.log.snapshot_and_follow();
        while !self.store.log.with_state(&done) {
            match tokio::time::timeout(PATIENCE, events.recv()).await {
                Ok(Some(_)) => {}
                _ => panic!(
                    "timed out waiting until {what}; the model was asked {} times, the tools \
                     refused {:?}",
                    self.provider.requests().len(),
                    self.desk.refusals()
                ),
            }
        }
    }

    /// The builder's message opens the CEO's task; the model's replies, which name it, are queued
    /// before the CEO's driver may start.
    async fn brief(&self, replies: impl FnOnce(TaskId) -> Vec<Reply>) -> TaskId {
        self.orch
            .send_chat(
                self.project,
                "Write the BRD, then report.".into(),
                vec![],
                None,
            )
            .await
            .unwrap();
        let task = self
            .store
            .log
            .with_state(|s| s.tasks().map(|t| t.task.id).last())
            .expect("the message opened a task");
        for reply in replies(task) {
            self.provider.reply(reply);
        }
        let allow = ProjectEvent::SettingChanged(SettingChange::Allow {
            what: self.model.clone(),
            allowed: true,
        });
        self.store.log.append(Actor::Builder, allow).unwrap();
        task
    }

    /// The BRD is written and the task reported, both by the CEO's one driver, through MCP.
    async fn check(self, task: TaskId) {
        self.until("the brief's task is done", |s| {
            matches!(s.task(task).map(|t| &t.state), Some(TaskState::Done(..)))
        })
        .await;
        self.until("the turn ends", |s| {
            s.drivers()
                .all(|d| d.state != DriverState::Running && d.state != DriverState::Starting)
        })
        .await;
        let s = self.store.log.state();
        let drivers: Vec<DriverId> = s.agent(self.ceo).unwrap().drivers.clone();
        assert_eq!(drivers.len(), 1, "one driver did it all");
        let driver = drivers[0];
        assert_eq!(s.driver(driver).unwrap().driver.harness, HARNESS);

        let brd = s.spec(SpecKind::Brd).expect("the BRD is written");
        assert_eq!(brd.spec.body, BRD);
        assert_eq!(brd.spec.author, self.ceo);
        let Some(TaskState::Done(_, report)) = s.task(task).map(|t| &t.state) else {
            unreachable!()
        };
        assert_eq!(report.outcome, Outcome::Completed);
        assert_eq!(report.summary, "The BRD is written.");
        // In the order they were made, which is the log's.
        let log = self.store.log.read_after(None, 10_000).unwrap();
        let calls: Vec<(DriverId, &ToolCall, bool)> = log
            .iter()
            .filter_map(|e| match &e.event {
                ProjectEvent::ActionProposed(a) => Some(a),
                _ => None,
            })
            .filter_map(|a| match &a.request {
                Request::Tool { driver, call } => {
                    let carried_out = s.action(a.id).is_some_and(|r| r.error.is_none());
                    Some((*driver, call, carried_out))
                }
                _ => None,
            })
            .collect();
        assert!(
            matches!(
                calls.as_slice(),
                [
                    (d1, ToolCall::WriteSpec { .. }, true),
                    (d2, ToolCall::Report { .. }, true),
                ] if *d1 == driver && *d2 == driver
            ),
            "{calls:#?}"
        );
        assert_eq!(self.desk.refusals(), vec![]);
        assert_eq!(
            std::fs::read_to_string(self.dir.path().join("fernleaf/.x100/specs/BRD.md")).unwrap(),
            BRD
        );

        self.orch.shutdown().await;
        assert_eq!(
            self.provider.unused(),
            0,
            "every scripted reply was asked for"
        );
        assert_eq!(self.provider.proxy_attempts(), Vec::<String>::new());
        assert_eq!(
            std::fs::read_to_string(self.dir.path().join("drivers.json")).unwrap(),
            "{}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_code_writes_the_brd_and_reports_through_mcp() {
    let Some(program) = installed("claude") else {
        return;
    };
    let run = Run::start(
        "claude-haiku-5-5",
        |provider, clock| {
            let mut config = ClaudeCodeConfig::new();
            config.program = program;
            config.base_url = Some(provider.url());
            config.inherit_env = vec!["PATH".into()];
            Arc::new(ClaudeCode::new(config, clock))
        },
        ("ANTHROPIC_API_KEY", "sk-ant-x100-fake-key"),
    )
    .await;
    let task = run
        .brief(|task| {
            vec![
                Reply::Tool {
                    name: "mcp__x100__write_spec".into(),
                    input: json!({ "spec": "brd", "status": "draft", "body": BRD }),
                },
                Reply::Tool {
                    name: "mcp__x100__report".into(),
                    input: json!({
                        "task": task,
                        "outcome": "completed",
                        "summary": "The BRD is written.",
                    }),
                },
                Reply::Text("Done: the BRD is written and the task reported.".into()),
            ]
        })
        .await;
    run.check(task).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_writes_the_brd_and_reports_through_mcp_from_its_exec_tool() {
    let Some(program) = installed("codex") else {
        return;
    };
    let run = Run::start(
        "gpt-6-luna",
        |provider, clock| {
            let mut config = CodexConfig::new();
            config.program = program;
            config.base_url = format!("{}/v1", provider.url());
            config.inherit_env = vec!["PATH".into()];
            Arc::new(Codex::new(config, clock))
        },
        ("OPENAI_API_KEY", "sk-x100-fake-openai-key"),
    )
    .await;
    // On gpt-6-luna Codex gives the model one JavaScript `exec` tool, through which it calls
    // MCP tools as functions of `tools`.
    let exec = |code: String| Reply::Freeform {
        name: "exec".into(),
        input: code,
    };
    let task = run
        .brief(|task| {
            let spec = json!({ "spec": "brd", "status": "draft", "body": BRD });
            let report = json!({
                "task": task,
                "outcome": "completed",
                "summary": "The BRD is written.",
            });
            vec![
                exec(format!(
                    "const r = await tools.mcp__x100__write_spec({spec}); text(JSON.stringify(r));"
                )),
                exec(format!(
                    "const r = await tools.mcp__x100__report({report}); text(JSON.stringify(r));"
                )),
                Reply::Text("Done: the BRD is written and the task reported.".into()),
            ]
        })
        .await;
    run.check(task).await;
}
