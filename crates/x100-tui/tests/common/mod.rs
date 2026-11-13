//! A real `x100d` as a child process on the fake clock, running the Fernleaf script, and helpers to
//! read what the interface shows.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// One rule for every package that starts a daemon: see the file.
#[path = "../../../x100-sandbox/tests/common/available.rs"]
pub mod available;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use x100_client::{Client, Options};
use x100_core::action::AskMeAbout;
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::id::ProjectId;
use x100_core::money::{Money, Percent};
use x100_core::project::Settings;
use x100_core::settings::DaemonSetting;
use x100_core::time::Timestamp;
use x100_proto::{CreateProject, GetProject, SetDaemonSetting};
use x100_tui::remember::Remember;
use x100_tui::run::Tui;

pub const START: Timestamp = Timestamp::from_millis(1_794_387_600_000);
pub const SEED: u64 = 7;
/// Bounds a hang; a whole run takes seconds.
pub const WAIT: Duration = Duration::from_secs(60);
pub const BRIEF: &str =
    "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.";
const SCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../x100-orch/tests/scripts");

/// The daemon's binaries, built by this workspace. A test of this package cannot name another
/// package's binary, so they are found beside the test's own executable, and built if missing.
pub fn binary(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("the test's path");
    let dir = exe
        .parent()
        .and_then(Path::parent)
        .expect("target/<profile>/deps");
    let path = dir.join(name);
    if !path.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "x100d", "--bins"])
            .status()
            .expect("cargo runs");
        assert!(status.success(), "building x100d failed");
    }
    path
}

pub struct Daemon {
    pub dir: tempfile::TempDir,
    child: Option<Child>,
}

impl Daemon {
    /// A daemon that runs the Fernleaf script's harness, on a clock that moves only when told.
    pub fn fernleaf() -> Daemon {
        // Under the build's own directory, not `/tmp`, which a sandbox replaces with its own.
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("a temporary directory");
        let child = Command::new(binary("x100d"))
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            // Code directories go under the test's own directory, never the builder's `~/x100`.
            .arg("--projects-root")
            .arg(dir.path().join("projects"))
            .arg("--scripts-dir")
            .arg(SCRIPTS)
            .arg("--fake-harness")
            .arg(binary("x100d-fake-harness"))
            .arg("--fake-clock")
            .arg(START.as_millis().to_string())
            // The same looks for the same team on every run.
            .arg("--seed")
            .arg(SEED.to_string())
            // The vault's key is a file in the test's data directory, never in a keychain.
            .env("X100_NO_KEYCHAIN", "1")
            .env("X100_DOCKER", "/nonexistent/x100-test/docker")
            .env("X100_GH", "/nonexistent/x100-test/gh")
            .stdin(Stdio::null())
            .spawn()
            .expect("x100d starts");
        Daemon {
            dir,
            child: Some(child),
        }
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.path().join("data/x100d.sock")
    }

    /// SIGTERM lets the daemon stop its drivers; SIGKILL would leave them behind.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status();
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until {
                if let Ok(Some(_)) = child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The daemon, a Tokio runtime for the client, and a client on it.
pub struct Run {
    pub rt: tokio::runtime::Runtime,
    pub daemon: Daemon,
    pub client: Client,
}

impl Run {
    pub fn start() -> Run {
        let daemon = Daemon::fernleaf();
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        let mut options = Options::new(daemon.socket(), "test");
        options.startup_timeout = WAIT;
        let client = rt
            .block_on(Client::connect(options))
            .expect("the daemon answers");
        // Where no sandbox can be made, drivers run without one so these tests can run at all.
        if !available::sandbox_works() {
            rt.block_on(client.request(SetDaemonSetting {
                setting: DaemonSetting::RunWithoutSandbox(true),
            }))
            .expect("the setting is taken");
        }
        Run { rt, daemon, client }
    }

    /// Waits long enough not to expire when the clock moves, as the daemon's Fernleaf test does.
    pub fn long_waits(&self) {
        self.rt
            .block_on(self.client.request(SetDaemonSetting {
                setting: DaemonSetting::DefaultWaitDeadline(Duration::from_secs(7 * 86_400)),
            }))
            .expect("the setting is taken");
    }

    /// A new project for the Fernleaf script, every agent on the script's short-lived model. Its
    /// first message is the test's to send.
    pub fn fernleaf_project(&self) -> ProjectId {
        self.long_waits();
        let client = &self.client;
        self.rt.block_on(async {
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
                trial_before_commit: false,
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
            client
                .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|_| ()))
                .await
                .expect("the client holds it");
            project
        })
    }

    /// Waits until the client holds everything the daemon has written and nothing more happens
    /// without the builder or the clock.
    pub fn settle(&self, project: ProjectId) {
        let client = &self.client;
        self.rt.block_on(async {
            let after = client
                .request(GetProject { project })
                .await
                .expect("the project")
                .seq();
            client
                .wait_for(WAIT, |s| {
                    let p = s.projects.get(&project)?;
                    (p.seq() >= after && quiet(p)).then_some(())
                })
                .await
                .expect("the run settles");
        });
    }

    /// An interface on this run's client, as the full screen is, without a terminal.
    pub fn tui(&self, remember: Remember) -> Screen {
        let tui = Tui::new(self.client.clone(), self.rt.handle().clone(), remember);
        Screen {
            tui,
            terminal: Terminal::new(TestBackend::new(200, 60)).expect("a test terminal"),
        }
    }
}

/// No turn running or starting, and nothing in any inbox waiting for one.
pub fn quiet(s: &ProjectState) -> bool {
    use x100_core::driver::DriverState;
    let busy = s.drivers().any(|d| {
        matches!(
            d.state,
            DriverState::Starting | DriverState::Running | DriverState::Compacting
        )
    });
    !busy && s.agents().all(|a| a.inbox.waiting().next().is_none())
}

/// The interface and a terminal it draws on.
pub struct Screen {
    pub tui: Tui,
    terminal: Terminal<TestBackend>,
}

impl Screen {
    /// What the screen shows now, one string per row with trailing blanks cut.
    pub fn text(&mut self) -> String {
        self.tui.drain();
        self.terminal.draw(|f| self.tui.app.draw(f)).expect("draws");
        let buffer = self.terminal.backend().buffer().clone();
        let width = buffer.area.width as usize;
        buffer
            .content()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn press(&mut self, key: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};
        self.tui.app.key(KeyEvent::new(key, KeyModifiers::NONE));
    }

    pub fn ctrl(&mut self, c: char) {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        self.tui
            .app
            .key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(ratatui::crossterm::event::KeyCode::Char(c));
        }
    }

    /// Redraws until `done` holds for the screen, applying what arrives meanwhile.
    pub fn wait_for(
        &mut self,
        rt: &tokio::runtime::Runtime,
        what: &str,
        mut done: impl FnMut(&str) -> bool,
    ) -> String {
        let until = Instant::now() + WAIT;
        loop {
            let _ = rt;
            std::thread::sleep(Duration::from_millis(5));
            let text = self.text();
            if done(&text) {
                return text;
            }
            assert!(
                Instant::now() < until,
                "timed out waiting until {what}; the screen shows\n{text}"
            );
        }
    }
}
