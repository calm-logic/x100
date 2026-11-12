//! A real `x100d` as a child process on the fake clock, with the Fernleaf script, and the app's
//! window on GPUI's test platform: no display, simulated clicks and keys, every frame drawn.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use gpui::{Bounds, Modifiers, Pixels, TestAppContext, VisualTestContext, WindowHandle};
use x100_app::shell::Shell;
use x100_client::{Client, Options};
use x100_core::action::AskMeAbout;
use x100_core::driver::DriverState;
use x100_core::fold::ProjectState;
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::id::ProjectId;
use x100_core::money::{Money, Percent};
use x100_core::project::Settings;
use x100_core::settings::DaemonSetting;
use x100_core::time::Timestamp;
use x100_proto::{CreateProject, GetProject, SetDaemonSetting};

pub const START: Timestamp = Timestamp::from_millis(1_794_387_600_000);
/// Bounds a hang; a whole run takes seconds.
pub const WAIT: Duration = Duration::from_secs(60);
const SCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../x100-orch/tests/scripts");

/// The daemon's binaries, built by this workspace. A test of this package cannot name another
/// package's binary, so they are found beside the test's own executable, and built if missing.
fn binary(name: &str) -> PathBuf {
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
        let dir = tempfile::tempdir().expect("a temporary directory");
        let child = Command::new(binary("x100d"))
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .arg("--scripts-dir")
            .arg(SCRIPTS)
            .arg("--fake-harness")
            .arg(binary("x100d-fake-harness"))
            .arg("--fake-clock")
            .arg(START.as_millis().to_string())
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

    pub fn project_dir(&self, name: &str) -> String {
        self.dir.path().join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // SIGKILL leaves driver processes behind; SIGTERM lets the daemon stop them.
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

/// The daemon, a Tokio runtime for clients, and the app's GPUI test context.
pub struct Run {
    pub rt: tokio::runtime::Runtime,
    pub daemon: Daemon,
    pub cx: TestAppContext,
}

impl Run {
    pub fn start() -> Run {
        let rt = tokio::runtime::Runtime::new().expect("a runtime");
        let daemon = Daemon::fernleaf();
        let cx = TestAppContext::single();
        cx.update(x100_app::init);
        Run { rt, daemon, cx }
    }

    pub fn client(&self) -> Client {
        let mut options = Options::new(self.daemon.socket(), "test");
        options.startup_timeout = WAIT;
        self.rt
            .block_on(Client::connect(options))
            .expect("the daemon answers")
    }

    /// Creates the Fernleaf project as the daemon's own Fernleaf test does: every agent on the
    /// script's short-lived model, and waits long enough not to expire when the clock moves.
    pub fn fernleaf_project(&self, client: &Client) -> ProjectId {
        self.rt.block_on(async {
            client
                .request(SetDaemonSetting {
                    setting: DaemonSetting::DefaultWaitDeadline(Duration::from_secs(7 * 86_400)),
                })
                .await
                .expect("the setting is taken");
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
                    path: self.daemon.project_dir("fernleaf"),
                    name: Some("Fernleaf".into()),
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
    /// without the builder or the clock, as the daemon's Fernleaf test does.
    pub fn settle(&self, client: &Client, project: ProjectId) {
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

    pub fn open(&mut self, client: Client) -> (WindowHandle<Shell>, VisualTestContext) {
        let window = self
            .cx
            .update(|cx| x100_app::open_window(cx, client))
            .expect("a window");
        let vcx = VisualTestContext::from_window(window.into(), &self.cx);
        (window, vcx)
    }
}

/// No turn running or starting, and nothing in any inbox waiting for one.
pub fn quiet(s: &ProjectState) -> bool {
    let busy = s.drivers().any(|d| {
        matches!(
            d.state,
            DriverState::Starting | DriverState::Running | DriverState::Compacting
        )
    });
    !busy && s.agents().all(|a| a.inbox.waiting().next().is_none())
}

/// Lets the app take in what the client has received: its tasks run on GPUI's test scheduler,
/// woken from the client's Tokio threads, so this runs them and gives those threads time.
pub fn pump(vcx: &mut VisualTestContext) {
    for _ in 0..10 {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    vcx.run_until_parked();
}

/// Pumps until `done` holds.
pub fn pump_until(
    vcx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let until = Instant::now() + WAIT;
    loop {
        pump(vcx);
        if done(vcx) {
            return;
        }
        assert!(Instant::now() < until, "timed out waiting until {what}");
    }
}

pub fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Option<Bounds<Pixels>> {
    vcx.debug_bounds(selector)
}

pub fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let b = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing drawn as {selector}"));
    vcx.simulate_click(b.center(), Modifiers::none());
    pump(vcx);
}
