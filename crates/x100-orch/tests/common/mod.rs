//! A project on disk with the orchestrator running it in this process: fake-harness child
//! processes playing a script, a real store and git in a temporary directory, the fake clock, and
//! one turn at a time so the log comes out the same on every run.
#![allow(dead_code)]

pub mod mcp;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use x100_core::driver::{DriverState, Incompatible};
use x100_core::effort::EffortRange;
use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessKind, HarnessModel, ModelName, ModelOffer, Support};
use x100_core::id::{AgentId, DriverId, HarnessId, MachineId, ProjectId, TaskId};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::Settings as ProjectSettings;
use x100_core::settings::DaemonSettings;
use x100_core::time::{Clock, FakeClock, Timestamp};
use x100_drivers::fake::{DaemonTools, FakeHarness};
use x100_drivers::{DriverHandle, DriverSpec, Harness, StartError, ToolEndpoint};
use x100_jobs::Scheduler;
use x100_orch::{
    AllowAll, Choice, Config, Desk, DriverChooser, NoSandbox, Orchestrator, ProcessRegistry,
    ToolServer,
};
use x100_store::ProjectStore;

pub const FAKE: &str = env!("CARGO_BIN_EXE_x100-orch-fake-harness");

/// The scripts name times on this clock, so a run is the same however long it takes.
pub const START: Timestamp = Timestamp::from_millis(1_794_387_600_000);
pub const WAIT: Duration = Duration::from_secs(60);
pub const HOUR: Duration = Duration::from_secs(3600);

pub struct World {
    pub dir: tempfile::TempDir,
    pub clock: FakeClock,
    pub store: ProjectStore,
    pub project: ProjectId,
    pub ceo: AgentId,
    pub orch: Orchestrator,
    pub scheduler: Scheduler<ProjectState>,
    pub desk: Arc<Desk>,
    /// Serving the tools over MCP, when the setup asked for it.
    pub tools: Option<ToolServer>,
    /// Where each driver was told to reach the tools, as its harness was given it.
    pub endpoints: Endpoints,
}

pub type Endpoints = Arc<Mutex<HashMap<DriverId, ToolEndpoint>>>;

/// One script; which model each role runs on, by default the one listed first.
pub struct Setup<'a> {
    pub script: &'a str,
    pub models: &'a [(&'a str, &'a str)],
    pub settings: DaemonSettings,
    /// Serve the tools over MCP as well as in-process.
    pub serve: bool,
}

impl<'a> Setup<'a> {
    pub fn new(script: &'a str) -> Self {
        // Long enough that a test moving the clock a day on expires no wait it did not mean to.
        let settings = DaemonSettings {
            default_wait_deadline: 7 * 24 * HOUR,
            ..DaemonSettings::default()
        };
        Self {
            script,
            models: &[],
            settings,
            serve: false,
        }
    }
}

const HARNESS: HarnessId = HarnessId::from_u128(7);

impl World {
    pub async fn new(setup: Setup<'_>) -> World {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let project_dir = dir.path().join("fernleaf");
        let clock = FakeClock::new(START);
        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        let store = ProjectStore::open(&project_dir, Arc::clone(&shared)).expect("a store");
        store.git.prepare().expect("a repository");
        let script = dir.path().join("script.toml");
        std::fs::write(&script, setup.script).expect("the script");

        let models = x100_drivers::fake::script::Script::parse(setup.script)
            .expect("the script parses")
            .offers();
        let allowed = models
            .iter()
            .map(|m| HarnessModel {
                harness: HARNESS,
                model: m.model.clone(),
            })
            .collect();
        let ceo = x100_orch::new_id(&*shared, AgentId::new);
        let created = ProjectEvent::Created {
            name: "Fernleaf".into(),
            ceo,
            charter: "Own Fernleaf's launch, from brief to a live site.".into(),
            effort: EffortRange::FULL,
            settings: ProjectSettings {
                ask_me_about: x100_core::action::AskMeAbout::HighRiskOnly,
                budget: Money::from_dollars(50),
                lab_share: Percent::new(10).unwrap(),
                allowed,
            },
        };
        store.log.append(Actor::Builder, created).expect("created");

        let settings = setup.settings;
        let settings: x100_orch::Settings = Arc::new(move || settings);
        let desk = Desk::new(Arc::new(AllowAll), Arc::clone(&settings));
        let tools = match setup.serve {
            true => Some(
                ToolServer::start(Arc::clone(&desk))
                    .await
                    .expect("a tools server"),
            ),
            false => None,
        };
        let fake = FakeHarness::new(
            FAKE,
            &script,
            Arc::clone(&shared),
            Some(Arc::clone(&desk) as Arc<dyn DaemonTools>),
        )
        .expect("the fake harness");
        let endpoints = Endpoints::default();
        let harness = Recording {
            inner: Arc::new(fake),
            endpoints: Arc::clone(&endpoints),
        };
        let default_model = models[0].model.clone();
        let by_role: HashMap<String, ModelName> = setup
            .models
            .iter()
            .map(|(role, model)| (role.to_string(), ModelName(model.to_string())))
            .collect();
        let (processes, _) =
            ProcessRegistry::open_and_reap(dir.path().join("drivers.json")).expect("registry");
        let orch = Orchestrator::new(Config {
            clock: Arc::clone(&shared),
            harnesses: HashMap::from([(
                HARNESS,
                Arc::new(harness) as Arc<dyn x100_drivers::Harness>,
            )]),
            chooser: Arc::new(ByRole {
                default: default_model,
                by_role,
            }),
            settings,
            processes: Arc::new(processes),
            desk: Arc::clone(&desk),
            sandbox: Arc::new(NoSandbox),
            serial: true,
        });
        let project = ProjectId::from_u128(1);
        let scheduler = Scheduler::new(store.log.clone(), shared);
        orch.attach(project, store.clone(), scheduler.clone());
        World {
            dir,
            clock,
            store,
            project,
            ceo,
            orch,
            scheduler,
            desk,
            tools,
            endpoints,
        }
    }

    pub fn state(&self) -> ProjectState {
        self.store.log.state()
    }

    pub fn project_dir(&self) -> PathBuf {
        self.store.git.project_dir().to_path_buf()
    }

    pub async fn brief(&self, body: &str) -> TaskId {
        self.orch
            .send_brief(self.project, body.into(), vec![])
            .await
            .expect("the brief is sent")
            .1
    }

    /// Waits until nothing more happens without the builder or the clock: no turn running or
    /// starting, and nothing in any inbox waiting for one.
    pub async fn settle(&self) {
        self.until("everything settles", quiet).await;
    }

    /// Waits, following the log, until `done` holds.
    pub async fn until(&self, what: &str, done: impl Fn(&ProjectState) -> bool) {
        let (_, mut events) = self.store.log.snapshot_and_follow();
        loop {
            if self.store.log.with_state(&done) {
                return;
            }
            match tokio::time::timeout(WAIT, events.recv()).await {
                Ok(Some(_)) => {}
                _ => panic!("timed out waiting until {what}\n{}", self.summary()),
            }
        }
    }

    /// Moves the clock to `at` and fires what is due, acting on each job as the daemon does.
    pub async fn clock_to(&self, at: Timestamp) {
        self.clock.set(at).expect("the clock only moves on");
        let scheduler = self.scheduler.clone();
        let fired = tokio::task::spawn_blocking(move || {
            let mut fired = Vec::new();
            scheduler.fire_due(|f| fired.push(f)).expect("jobs fire");
            fired
        })
        .await
        .unwrap();
        for f in fired {
            self.orch
                .job_fired(self.project, f.job, f.at)
                .await
                .expect("the orchestrator acts on the job");
        }
    }

    pub fn log(&self) -> Vec<Envelope<ProjectEvent>> {
        self.store.log.read_after(None, 100_000).expect("the log")
    }

    /// The log read back from the store folds into the state the store holds.
    pub fn assert_log_folds(&self) {
        let folded = ProjectState::fold(&self.log()).expect("the log folds");
        assert_eq!(
            folded,
            self.state(),
            "the stored log folds into the live state"
        );
    }

    pub fn agent(&self, role: &str) -> AgentId {
        agent(&self.state(), role)
    }

    /// Stops every driver and checks that no process of ours is left.
    pub async fn finish(self) {
        self.orch.shutdown().await;
        let left =
            std::fs::read_to_string(self.dir.path().join("drivers.json")).unwrap_or_default();
        assert!(
            left.trim() == "{}" || left.is_empty(),
            "drivers left: {left}"
        );
        self.assert_log_folds();
    }

    pub fn summary(&self) -> String {
        let s = self.state();
        let mut out = String::new();
        for a in s.agents() {
            let drivers: Vec<_> = a
                .drivers
                .iter()
                .filter_map(|d| s.driver(*d))
                .map(|d| d.state.name())
                .collect();
            out.push_str(&format!(
                "{} {:?} waiting={} waits={} tasks={} drivers={drivers:?}\n",
                a.role,
                a.state.name(),
                a.inbox.waiting().count(),
                a.waits.len(),
                a.open_tasks.len()
            ));
        }
        let failed: Vec<_> = s.actions().filter_map(|a| a.error.clone()).collect();
        out.push_str(&format!("failed calls: {failed:#?}\n"));
        out
    }
}

pub fn agent(state: &ProjectState, role: &str) -> AgentId {
    state
        .agents()
        .find(|a| a.role == role)
        .map(|a| a.id)
        .unwrap_or_else(|| panic!("no agent {role}"))
}

pub fn quiet(s: &ProjectState) -> bool {
    let busy = s.drivers().any(|d| {
        matches!(
            d.state,
            DriverState::Starting | DriverState::Running | DriverState::Compacting
        )
    });
    !busy && s.agents().all(|a| a.inbox.waiting().next().is_none())
}

/// Each event's kind, for comparing runs whose ids and times differ.
pub fn kinds(log: &[Envelope<ProjectEvent>]) -> Vec<String> {
    log.iter().map(|e| kind(&e.event)).collect()
}

pub fn kind(e: &ProjectEvent) -> String {
    let value = serde_json::to_value(e).expect("events serialise");
    let outer = match &value {
        serde_json::Value::Object(m) => m.keys().next().cloned().unwrap_or_default(),
        serde_json::Value::String(s) => s.clone(),
        _ => String::new(),
    };
    match e {
        ProjectEvent::Agent { event, .. } => format!("Agent.{}", event.name()),
        ProjectEvent::Driver { event, .. } => format!("Driver.{}", event.name()),
        ProjectEvent::Task { event, .. } => format!("Task.{}", event.name()),
        ProjectEvent::Job { event, .. } => format!("Job.{}", event.name()),
        ProjectEvent::Action { event, .. } => format!("Action.{}", event.name()),
        _ => outer,
    }
}

/// Picks each role's model from the setup, at the lowest effort its range allows.
struct ByRole {
    default: ModelName,
    by_role: HashMap<String, ModelName>,
}

impl DriverChooser for ByRole {
    fn choose(&self, project: &ProjectState, agent: AgentId) -> Result<Choice, Vec<Incompatible>> {
        let a = project
            .agent(agent)
            .ok_or(vec![Incompatible::NothingAllowed])?;
        let range = project
            .project()
            .and_then(|p| p.team.effort(agent))
            .ok_or(vec![Incompatible::NoEffortInRange])?;
        Ok(Choice {
            harness: HARNESS,
            model: self
                .by_role
                .get(&a.role)
                .cloned()
                .unwrap_or(self.default.clone()),
            effort: range.min(),
            machine: MachineId::from_u128(1),
        })
    }
}

pub fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// A harness that notes the tools endpoint each driver is started with, as a test reading the
/// harness's side of the wire would.
pub struct Recording {
    pub inner: Arc<dyn Harness>,
    pub endpoints: Endpoints,
}

#[async_trait]
impl Harness for Recording {
    fn kind(&self) -> HarnessKind {
        self.inner.kind()
    }

    fn models(&self) -> Vec<ModelOffer> {
        self.inner.models()
    }

    fn supports(&self) -> Support {
        self.inner.supports()
    }

    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle, StartError> {
        if let Some(endpoint) = &spec.tools {
            self.endpoints
                .lock()
                .expect("endpoints lock")
                .insert(spec.driver, endpoint.clone());
        }
        self.inner.start(spec).await
    }
}
