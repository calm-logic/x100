//! The real `claude` and `codex` programs run by a real daemon as terminal drivers, each in its
//! agent's sandbox, with the model played by the fake provider on localhost. No request leaves the
//! machine and no real login is touched: each program's login is a dummy written here into a
//! throwaway directory the daemon is pointed at, the daemon's home and data directory are
//! throwaway too, its environment is built from nothing, its vault's key is a file, and the base
//! URLs name a made-up host that the daemon's proxy, the drivers' only way out, sends to the fake
//! provider while it refuses every other host. A test is skipped, saying so, when its program is
//! not installed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::sync::watch;
use x100_client::{Client, Options, TerminalStream};
use x100_core::action::{ActionState, AskMeAbout, Request};
use x100_core::agent::{AgentState, Waiting};
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::id::{AgentId, DriverId, HarnessId, ProjectId};
use x100_core::money::{Money, Percent};
use x100_core::project::{SettingChange, Settings};
use x100_core::terminal::{Chunk, TermPos};
use x100_core::time::Timestamp;
use x100_drivers::testing::FakeProvider;
use x100_proto::{CreateProject, ReadTerminal};

use super::{DaemonProc, WAIT};

/// A turn of a real program takes seconds; this only bounds a hang.
pub const PATIENCE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Claude,
    Codex,
}

impl Program {
    pub fn kind(self) -> HarnessKind {
        match self {
            Program::Claude => HarnessKind::ClaudeCodeTerminal,
            Program::Codex => HarnessKind::CodexTerminal,
        }
    }

    /// The cheapest model of each, which is what a new project allows by default.
    pub fn model(self) -> &'static str {
        match self {
            Program::Claude => "claude-haiku-5-5",
            Program::Codex => "gpt-6-luna",
        }
    }
}

/// The provider's made-up host, which the daemon's proxy sends to the fake provider.
pub const PROVIDER_HOST: &str = "provider.x100.test";

pub struct Real {
    pub dir: tempfile::TempDir,
    pub data: PathBuf,
    pub provider: FakeProvider,
    pub daemon: DaemonProc,
    pub client: Client,
    program: Program,
    env: Vec<(String, String)>,
}

fn installed(program: &str) -> bool {
    let found = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()));
    if !found {
        eprintln!("SKIPPED: {program} is not installed on this machine");
    }
    found
}

/// Dummy logins as each program writes them, good for hours.
fn write_logins(claude: &std::path::Path, codex: &std::path::Path) {
    std::fs::create_dir_all(claude).unwrap();
    std::fs::create_dir_all(codex).unwrap();
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 8 * 3600 * 1000;
    let credentials = json!({ "claudeAiOauth": {
        "accessToken": "sk-ant-oat01-x100-dummy-access-token",
        "refreshToken": "sk-ant-ort01-x100-dummy-refresh",
        "expiresAt": expires,
        "scopes": ["user:inference", "user:profile", "user:sessions:claude_code"],
        "subscriptionType": "max",
    }});
    std::fs::write(claude.join(".credentials.json"), credentials.to_string()).unwrap();
    let auth = json!({ "auth_mode": "apikey", "OPENAI_API_KEY": "sk-x100-fake-openai-key" });
    std::fs::write(codex.join("auth.json"), auth.to_string()).unwrap();
}

pub fn now() -> Timestamp {
    Timestamp::from_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    )
}

impl Real {
    pub async fn start(program: Program) -> Option<Real> {
        let name = match program {
            Program::Claude => "claude",
            Program::Codex => "codex",
        };
        if !installed(name) {
            return None;
        }
        // These are about drivers contained in a sandbox and what reaches out through the proxy.
        if super::available::skip_without_sandbox() {
            return None;
        }
        // Not under /tmp: Codex refuses to set up its sandbox's helpers in a home there, as it
        // would in the builder's data directory.
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let data = dir.path().join("data");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let (claude, codex) = (
            dir.path().join("claude-login"),
            dir.path().join("codex-login"),
        );
        write_logins(&claude, &codex);
        let provider = FakeProvider::start().await.unwrap();
        let text = |p: &PathBuf| p.display().to_string();
        let port = provider.url().rsplit(':').next().unwrap().to_owned();
        let base = format!("http://{PROVIDER_HOST}:{port}");
        let env = vec![
            ("PATH".into(), std::env::var("PATH").unwrap()),
            ("HOME".into(), text(&home)),
            ("LANG".into(), "C.UTF-8".into()),
            ("X100_CLAUDE_LOGIN".into(), text(&claude)),
            ("X100_CODEX_LOGIN".into(), text(&codex)),
            ("ANTHROPIC_BASE_URL".into(), base.clone()),
            ("OPENAI_BASE_URL".into(), format!("{base}/v1")),
            (
                x100d::PROXY_RESOLVE_ENV.into(),
                format!("{PROVIDER_HOST}=127.0.0.1"),
            ),
        ];
        let daemon = DaemonProc::spawn_in(args(&data), &env);
        let client = connect(&data).await;
        Some(Real {
            dir,
            data,
            provider,
            daemon,
            client,
            program,
            env,
        })
    }

    /// Another client of the same daemon.
    pub async fn another_client(&self) -> Client {
        connect(&self.data).await
    }

    /// A daemon started again on the same data directory and environment.
    pub async fn restart(&mut self) {
        self.daemon = DaemonProc::spawn_in(args(&self.data), &self.env);
        self.client = connect(&self.data).await;
    }

    /// Stops the daemon and starts it again with one variable of its environment changed.
    pub async fn restart_with(&mut self, key: &str, value: String) {
        self.daemon.terminate();
        for (k, v) in &mut self.env {
            if k == key {
                *v = value.clone();
            }
        }
        self.restart().await;
    }

    pub fn harness(&self) -> HarnessId {
        let kind = self.program.kind();
        self.client
            .with_state(|s| s.daemon.harnesses().find(|h| h.kind == kind).map(|h| h.id))
            .expect("the terminal harness is registered")
    }

    /// A new project that allows nothing yet, so the CEO's driver starts only once the replies
    /// are queued and [`Real::allow`] is called. A turn that neither reports nor waits gives its
    /// task up at once, so a reply that only talks is the turn's last.
    pub async fn project(&self) -> ProjectId {
        self.client
            .request(x100_proto::SetDaemonSetting {
                setting: x100_core::settings::DaemonSetting::TurnsWithoutProgress(1),
            })
            .await
            .unwrap();
        let settings = Settings {
            ask_me_about: AskMeAbout::HighRiskOnly,
            budget: Money::from_dollars(5),
            lab_share: Percent::new(10).unwrap(),
            trial_before_commit: false,
            allowed: Default::default(),
        };
        let project = self
            .client
            .request(CreateProject {
                dir: None,
                settings: Some(settings),
            })
            .await
            .expect("the project is created")
            .project;
        self.client
            .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|_| ()))
            .await
            .unwrap();
        project
    }

    pub async fn allow(&self, project: ProjectId) {
        let what = HarnessModel {
            harness: self.harness(),
            model: ModelName(self.program.model().into()),
        };
        let allow = SettingChange::Allow {
            what,
            allowed: true,
        };
        self.client.change_setting(project, allow).await.unwrap();
    }

    pub async fn until<T>(&self, what: &str, f: impl Fn(&ProjectState) -> Option<T>) -> T {
        let project = self
            .client
            .with_state(|s| s.projects.keys().next().copied());
        let project = project.expect("a project");
        self.client
            .wait_for(PATIENCE, |s| f(s.projects.get(&project)?))
            .await
            .unwrap_or_else(|_| {
                let drivers: Vec<_> = self
                    .client
                    .project(project)
                    .map(|p| p.drivers().map(|d| d.state.clone()).collect())
                    .unwrap_or_default();
                panic!(
                    "timed out waiting until {what}; the model was asked {} times; drivers: \
                     {drivers:?}",
                    self.provider.requests().len()
                )
            })
    }

    /// Where the daemon makes code directories.
    pub fn projects_root(&self) -> PathBuf {
        self.dir.path().join("projects")
    }

    pub async fn ceo_waits_for_a_driver(&self) -> AgentId {
        self.until("the CEO waits for a driver", |p| {
            let ceo = p.project()?.ceo;
            matches!(
                p.agent(ceo)?.state,
                AgentState::Waiting(Waiting::ForDriver(_))
            )
            .then_some(ceo)
        })
        .await
    }

    /// Every request a driver sent out went through the proxy, and every one to a host but the
    /// provider's was refused there: the log records each as an egress action.
    pub fn assert_nothing_else_left_the_machine(&self, project: ProjectId) {
        let s = self.client.project(project).unwrap();
        let egress: Vec<(String, bool)> = s
            .actions()
            .filter_map(|a| match &a.action.request {
                Request::Egress { host, .. } => {
                    let refused = a.state == ActionState::Failed;
                    Some((host.clone(), refused))
                }
                _ => None,
            })
            .collect();
        assert!(
            egress.iter().any(|(h, _)| h == PROVIDER_HOST),
            "the provider was reached through the proxy: {egress:?}"
        );
        assert!(
            egress
                .iter()
                .all(|(h, refused)| h == PROVIDER_HOST || *refused),
            "{egress:?}"
        );
        assert!(self.provider.proxy_attempts().is_empty());
    }
}

/// The data directory and, beside it, the projects root, both inside the test's directory.
fn args(data: &std::path::Path) -> Vec<std::ffi::OsString> {
    let root = data
        .parent()
        .expect("the test's directory")
        .join("projects");
    vec![
        "--data-dir".into(),
        data.into(),
        "--projects-root".into(),
        root.into(),
    ]
}

async fn connect(data: &std::path::Path) -> Client {
    let mut options = Options::new(data.join("x100d.sock"), "test");
    options.startup_timeout = WAIT;
    Client::connect(options).await.expect("the daemon answers")
}

/// Every chunk of a driver's terminal so far, as `terminal.read` pages it from the start.
pub async fn read_all(
    client: &Client,
    project: ProjectId,
    driver: DriverId,
) -> Vec<(TermPos, Chunk)> {
    let mut out: Vec<(TermPos, Chunk)> = Vec::new();
    loop {
        let after = out.last().map(|(p, _)| *p);
        let page = client
            .request(ReadTerminal {
                project,
                driver,
                after,
                limit: 1000,
            })
            .await
            .unwrap();
        if page.chunks.is_empty() {
            return out;
        }
        out.extend(page.chunks.into_iter().map(|c| (c.position, c.chunk)));
    }
}

/// A terminal subscription read into a list as it delivers, each chunk with when it arrived.
pub struct Watch {
    seen: Arc<Mutex<Vec<(TermPos, Chunk, Timestamp)>>>,
    count: watch::Receiver<usize>,
    task: tokio::task::JoinHandle<()>,
}

impl Watch {
    pub fn new(mut stream: TerminalStream) -> Watch {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (tx, count) = watch::channel(0);
        let into = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            while let Some((position, chunk)) = stream.next().await {
                let mut seen = into.lock().unwrap();
                seen.push((position, chunk, now()));
                let _ = tx.send(seen.len());
            }
        });
        Watch { seen, count, task }
    }

    /// Waits until at least `n` chunks have arrived.
    pub async fn until(&mut self, n: usize) {
        let reached = tokio::time::timeout(PATIENCE, self.count.wait_for(|c| *c >= n)).await;
        assert!(
            matches!(reached, Ok(Ok(_))),
            "only {} of {n} chunks arrived",
            self.seen.lock().unwrap().len()
        );
    }

    pub fn seen(&self) -> Vec<(TermPos, Chunk, Timestamp)> {
        self.seen.lock().unwrap().clone()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}
