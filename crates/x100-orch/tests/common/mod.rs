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
use x100_core::driver::DriverState;
use x100_core::effort::EffortRange;
use x100_core::event::{DaemonEvent, Envelope, ProjectEvent};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::harness::{HarnessKind, HarnessModel, Machine, ModelOffer, Support};
use x100_core::id::{AgentId, DriverId, HarnessId, MachineId, ProjectId, TaskId};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::Settings as ProjectSettings;
use x100_core::settings::{DaemonSetting, DaemonSettings};
use x100_core::time::{Clock, FakeClock, Timestamp};
use x100_drivers::fake::{DaemonTools, FakeHarness};
use x100_drivers::{DriverHandle, DriverSpec, Harness, StartError, ToolEndpoint};
use x100_jobs::Scheduler;
use x100_orch::{AllowAll, Config, Desk, NoSandbox, Orchestrator, ProcessRegistry, ToolServer};
use x100_store::{DaemonLog, ProjectStore};

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
    /// The daemon's log, which the router reads and cool-offs are written to, and its job clock.
    pub daemon: DaemonLog,
    pub daemon_jobs: Scheduler<DaemonState>,
    /// Each provider's harness, in the order the setup named its script.
    pub harnesses: Vec<HarnessId>,
    pub desk: Arc<Desk>,
    /// Serving the tools over MCP, when the setup asked for it.
    pub tools: Option<ToolServer>,
    /// Where each driver was told to reach the tools, as its harness was given it.
    pub endpoints: Endpoints,
}

pub type Endpoints = Arc<Mutex<HashMap<DriverId, ToolEndpoint>>>;

/// One script, and more for more providers. Every model of every provider is allowed unless the
/// setup names which.
pub struct Setup<'a> {
    pub script: &'a str,
    pub providers: &'a [&'a str],
    pub allowed: Option<&'a [(usize, &'a str)]>,
    pub budget: Money,
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
            providers: &[],
            allowed: None,
            budget: Money::from_dollars(50),
            settings,
            serve: false,
        }
    }
}

pub const MACHINE: MachineId = MachineId::from_u128(1);

/// The harness id of the setup's `n`th script.
pub fn harness_id(n: usize) -> HarnessId {
    HarnessId::from_u128(7 + n as u128)
}

/// A daemon's log in `dir` holding the machine and these settings, with its job clock: what the
/// router reads. Harnesses are added once they exist, with [`add_harnesses`].
pub fn daemon_log(
    dir: &Path,
    clock: Arc<dyn Clock>,
    settings: &DaemonSettings,
) -> (DaemonLog, Scheduler<DaemonState>) {
    let log = x100_store::open_daemon_log(dir, Arc::clone(&clock)).expect("a daemon log");
    let mut events = vec![DaemonEvent::MachineAdded(Machine {
        id: MACHINE,
        name: "local".into(),
        harnesses: Vec::new(),
        cap: 16,
    })];
    events.extend(
        all_settings(settings)
            .into_iter()
            .map(DaemonEvent::SettingChanged),
    );
    log.append_all(events.into_iter().map(|e| (Actor::Daemon, e)).collect())
        .expect("the daemon's log takes them");
    let jobs = Scheduler::new(log.clone(), clock);
    (log, jobs)
}

/// Records harnesses in the daemon's log as the daemon's discovery would.
pub fn add_harnesses(log: &DaemonLog, harnesses: &HashMap<HarnessId, Arc<dyn Harness>>) {
    let mut ids: Vec<_> = harnesses.keys().copied().collect();
    ids.sort();
    let events = ids
        .into_iter()
        .map(|id| {
            let h = &harnesses[&id];
            let record = x100_core::harness::Harness {
                id,
                kind: h.kind(),
                support: h.supports(),
                models: h.models(),
            };
            (Actor::Daemon, DaemonEvent::HarnessAdded(record))
        })
        .collect();
    log.append_all(events).expect("the daemon's log takes them");
}

/// Every setting as a change, so a log holds exactly these values.
fn all_settings(s: &DaemonSettings) -> Vec<DaemonSetting> {
    use DaemonSetting as D;
    vec![
        D::RetireAtContext(s.retire_at_context),
        D::RetireAfterTasks(s.retire_after_tasks),
        D::RetireAfterAge(s.retire_after_age),
        D::StallTimeout(s.stall_timeout),
        D::DefaultWaitDeadline(s.default_wait_deadline),
        D::MinimumRepeat(s.minimum_repeat),
        D::StartPackShare(s.start_pack_share),
        D::LastTurnLimit(s.last_turn_limit),
        D::TurnsWithoutProgress(s.turns_without_progress),
        D::CoolOff(s.cool_off),
    ]
}

/// The daemon's settings as its log holds them now.
pub fn settings_of(log: &DaemonLog) -> x100_orch::Settings {
    let log = log.clone();
    Arc::new(move || log.with_state(|d| *d.settings()))
}

impl World {
    pub async fn new(setup: Setup<'_>) -> World {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let project_dir = dir.path().join("fernleaf");
        let clock = FakeClock::new(START);
        let shared: Arc<dyn Clock> = Arc::new(clock.clone());
        let store = ProjectStore::open(&project_dir, Arc::clone(&shared)).expect("a store");
        store.git.prepare().expect("a repository");
        let scripts: Vec<&str> = std::iter::once(setup.script)
            .chain(setup.providers.iter().copied())
            .collect();
        let mut allowed = std::collections::BTreeSet::new();
        for (n, text) in scripts.iter().enumerate() {
            let offers = x100_drivers::fake::script::Script::parse(text)
                .expect("the script parses")
                .offers();
            for m in offers {
                let named = setup
                    .allowed
                    .is_none_or(|a| a.iter().any(|(i, model)| *i == n && m.model.0 == *model));
                if named {
                    allowed.insert(HarnessModel {
                        harness: harness_id(n),
                        model: m.model.clone(),
                    });
                }
            }
        }
        let ceo = x100_orch::new_id(&*shared, AgentId::new);
        let created = ProjectEvent::Created {
            name: "Fernleaf".into(),
            ceo,
            charter: "Own Fernleaf's launch, from brief to a live site.".into(),
            effort: EffortRange::FULL,
            settings: ProjectSettings {
                ask_me_about: x100_core::action::AskMeAbout::HighRiskOnly,
                budget: setup.budget,
                lab_share: Percent::new(10).unwrap(),
                allowed,
            },
        };
        store.log.append(Actor::Builder, created).expect("created");

        let (daemon, daemon_jobs) = daemon_log(
            &dir.path().join("daemon"),
            Arc::clone(&shared),
            &setup.settings,
        );
        let settings = settings_of(&daemon);
        let desk = Desk::new(Arc::new(AllowAll), Arc::clone(&settings));
        let tools = match setup.serve {
            true => Some(
                ToolServer::start(Arc::clone(&desk))
                    .await
                    .expect("a tools server"),
            ),
            false => None,
        };
        let endpoints = Endpoints::default();
        let mut harnesses: HashMap<HarnessId, Arc<dyn x100_drivers::Harness>> = HashMap::new();
        for (n, text) in scripts.iter().enumerate() {
            let path = dir.path().join(format!("script-{n}.toml"));
            std::fs::write(&path, text).expect("the script");
            let fake = FakeHarness::new(
                FAKE,
                &path,
                Arc::clone(&shared),
                Some(Arc::clone(&desk) as Arc<dyn DaemonTools>),
            )
            .expect("the fake harness");
            let recording = Recording {
                inner: Arc::new(fake),
                endpoints: Arc::clone(&endpoints),
            };
            harnesses.insert(harness_id(n), Arc::new(recording));
        }
        let ids: Vec<HarnessId> = (0..scripts.len()).map(harness_id).collect();
        add_harnesses(&daemon, &harnesses);
        let (processes, _) =
            ProcessRegistry::open_and_reap(dir.path().join("drivers.json")).expect("registry");
        let orch = Orchestrator::new(Config {
            clock: Arc::clone(&shared),
            harnesses,
            machine: MACHINE,
            logins: Default::default(),
            daemon: daemon.clone(),
            daemon_jobs: daemon_jobs.clone(),
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
            daemon,
            daemon_jobs,
            harnesses: ids,
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
    /// starting, and nothing in any inbox waiting for one, but for an agent waiting for a driver.
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
        let jobs = self.daemon_jobs.clone();
        let fired = tokio::task::spawn_blocking(move || {
            let mut fired = Vec::new();
            jobs.fire_due(|f| fired.push(f))
                .expect("the daemon's jobs fire");
            fired
        })
        .await
        .unwrap();
        for f in fired {
            self.orch.daemon_job_fired(f.job).await;
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
    // An agent waiting for a driver keeps its items until something changes that could help.
    let waits_for_driver = |a: &x100_core::fold::AgentRecord| {
        matches!(
            a.state,
            x100_core::agent::AgentState::Waiting(x100_core::agent::Waiting::ForDriver(_))
        )
    };
    !busy
        && s.agents()
            .all(|a| a.inbox.waiting().next().is_none() || waits_for_driver(a))
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
