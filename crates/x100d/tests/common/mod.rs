//! A daemon as a child process on a socket in a temporary directory, and the client library as the
//! way in.
#![allow(dead_code)]

pub mod real;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use x100_client::{Client, Options};
use x100_core::fold::{DriverRecord, ProjectState};
use x100_core::id::{DriverId, ProjectId};
use x100_proto::{CreateProject, Snapshot};

pub const WAIT: Duration = Duration::from_secs(30);

pub const SCRIPT: &str = r#"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
# The CEO's task from the builder needs High, and a driver must meet it.
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0, cache_read = 0.1 }

# A turn that is still going when a test kills or stops the daemon.
[[turn]]
when = { input_contains = "SLEEP" }
repeat = true
# The background process stands for whatever a harness started and left running: it stays in the
# driver's process group when the driver itself is gone.
steps = [
  { run = "sleep 300 >/dev/null 2>&1 &" },
  { text = "going to sleep" },
  { sleep_ms = 120000 },
]

[[turn]]
when = { input_contains = "CRASH" }
repeat = true
steps = [{ text = "about to crash" }, { crash = 3 }]

[[turn]]
when = { input_contains = "TOOL" }
repeat = true
steps = [
  { call = { set_preview = { url = "http://localhost:1" } } },
  { text = "tried a tool" },
]

[[turn]]
when = { input_contains = "SLOW" }
repeat = true
steps = [
  { text = "thinking" },
  { sleep_ms = 700 },
  { call = { message = { to = "builder", body = "done thinking", attachments = [] } } },
]

# The CEO answers with `message`, as a model must: the text it writes is not a reply.
[[turn]]
when = { input_contains = "A message from the builder." }
repeat = true
steps = [
  { text = "reading the message" },
  { call = { message = { to = "builder", body = "heard: {{input}}", attachments = [], in_reply_to = "{{input:Message}}" } } },
  { write = "notes/seen.md", content = "seen" },
  { usage = { input = 100, output = 20, cost_micros = 1234 } },
  { context = 5000 },
]

# Anything else, such as a job waking it, is told to the builder too, as a reply to nothing.
[[turn]]
repeat = true
steps = [
  { text = "reading the input" },
  { call = { message = { to = "builder", body = "heard: {{input}}", attachments = [] } } },
  { write = "notes/seen.md", content = "seen" },
  { usage = { input = 100, output = 20, cost_micros = 1234 } },
  { context = 5000 },
]
"#;

pub struct Env {
    pub dir: tempfile::TempDir,
    pub data: PathBuf,
    pub scripts: PathBuf,
}

impl Env {
    pub fn new() -> Env {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let data = dir.path().join("data");
        let scripts = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("fake.toml"), SCRIPT).unwrap();
        Env { dir, data, scripts }
    }

    pub fn socket(&self) -> PathBuf {
        self.data.join("x100d.sock")
    }

    pub fn project_dir(&self, name: &str) -> String {
        self.dir.path().join(name).to_string_lossy().into_owned()
    }

    /// Where the daemon makes a project's code directory when it is set up.
    pub fn projects_root(&self) -> PathBuf {
        self.dir.path().join("projects")
    }

    pub fn daemon_args(&self, fake_harness: &Path) -> Vec<std::ffi::OsString> {
        vec![
            "--data-dir".into(),
            self.data.clone().into(),
            // Code directories go under the test's own directory, never the builder's `~/x100`.
            "--projects-root".into(),
            self.projects_root().into(),
            "--scripts-dir".into(),
            self.scripts.clone().into(),
            "--fake-harness".into(),
            fake_harness.into(),
        ]
    }

    pub fn spawn(&self) -> DaemonProc {
        self.spawn_with(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")))
    }

    /// A daemon whose drivers are started by `fake_harness` instead of the usual program.
    pub fn spawn_with(&self, fake_harness: &Path) -> DaemonProc {
        DaemonProc::spawn(self.daemon_args(fake_harness))
    }

    pub async fn client(&self) -> Client {
        let mut options = Options::new(self.socket(), "test");
        options.startup_timeout = WAIT;
        Client::connect(options).await.expect("the daemon answers")
    }

    /// The driver processes the daemon has written down, by driver id: one for most drivers,
    /// two for Codex in a terminal.
    pub fn recorded_pids(&self) -> Vec<(String, u32)> {
        let Ok(text) = std::fs::read_to_string(self.data.join("drivers.json")) else {
            return Vec::new();
        };
        let map: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        map.as_object()
            .map(|m| {
                m.iter()
                    .flat_map(|(k, v)| {
                        let entries = v.as_array().cloned().unwrap_or_default();
                        entries
                            .into_iter()
                            .map(move |e| (k.clone(), e["pid"].as_u64().unwrap() as u32))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub struct DaemonProc {
    child: Option<Child>,
}

/// A test's daemon makes code directories only where the test says, never in the builder's
/// `~/x100`.
fn own_projects_root(args: &[std::ffi::OsString]) {
    assert!(
        args.iter().any(|a| a == "--projects-root"),
        "a test's daemon needs --projects-root: {args:?}"
    );
}

impl DaemonProc {
    pub fn spawn(args: Vec<std::ffi::OsString>) -> DaemonProc {
        own_projects_root(&args);
        let child = Command::new(env!("CARGO_BIN_EXE_x100d"))
            .args(args)
            .stdin(Stdio::null())
            .spawn()
            .expect("x100d starts");
        DaemonProc { child: Some(child) }
    }

    /// A daemon with only the environment given, nothing of the test's own but what it names.
    pub fn spawn_in(args: Vec<std::ffi::OsString>, env: &[(String, String)]) -> DaemonProc {
        own_projects_root(&args);
        let child = Command::new(env!("CARGO_BIN_EXE_x100d"))
            .args(args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .spawn()
            .expect("x100d starts");
        DaemonProc { child: Some(child) }
    }

    pub fn pid(&self) -> u32 {
        self.child.as_ref().expect("running").id()
    }

    /// SIGKILL: the daemon can clean up nothing.
    pub fn kill9(&mut self) {
        let mut child = self.child.take().expect("running");
        child.kill().expect("a signal");
        child.wait().expect("reaped");
    }

    /// SIGTERM, and waits for a clean exit.
    pub fn terminate(&mut self) {
        let mut child = self.child.take().expect("running");
        kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("a signal");
        let status = child.wait().expect("reaped");
        assert!(status.success(), "the daemon exited with {status}");
    }
}

impl Drop for DaemonProc {
    fn drop(&mut self) {
        // A test that failed halfway must not leave the daemon running.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Whether the process is alive. A zombie waiting for its parent to collect it is not.
pub fn running(pid: u32) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// The live processes in a process group.
pub fn group_members(pgid: u32) -> Vec<u32> {
    let out = Command::new("ps")
        .args(["-eo", "pid=,pgid=,stat="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, group, stat) = (f.next()?, f.next()?, f.next()?);
            (group.parse() == Ok(pgid) && !stat.starts_with('Z')).then(|| pid.parse().ok())?
        })
        .collect()
}

pub async fn gone(pids: &[u32]) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while pids.iter().any(|p| running(*p)) {
        assert!(std::time::Instant::now() < until, "still running: {pids:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Creates a project and waits for the client to hold it.
///
/// These scripts never report or wait. With the daemon's usual limit the CEO would be nudged for
/// three turns before its task is given up; one turn keeps each exchange to the turn a test is
/// about.
pub async fn create_project(client: &Client, env: &Env, name: &str) -> ProjectId {
    client
        .request(x100_proto::SetDaemonSetting {
            setting: x100_core::settings::DaemonSetting::TurnsWithoutProgress(1),
        })
        .await
        .expect("the setting is taken");
    // On a folder of its own, so the CEO works in a worktree from its first turn.
    std::fs::create_dir_all(env.project_dir(name)).expect("the project's folder");
    let project = client
        .request(CreateProject {
            dir: Some(env.project_dir(name)),
            settings: None,
        })
        .await
        .expect("the project is created")
        .project;
    client
        .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|_| ()))
        .await
        .expect("the client follows the new project");
    project
}

pub fn drivers(s: &Snapshot, project: ProjectId) -> Vec<DriverRecord> {
    s.projects
        .get(&project)
        .map(|p| p.drivers().cloned().collect())
        .unwrap_or_default()
}

pub fn ceo_driver(p: &ProjectState) -> Option<&DriverRecord> {
    let ceo = p.project()?.ceo;
    p.agent(ceo)?.drivers.last().and_then(|d| p.driver(*d))
}

pub fn only_driver(s: &Snapshot, project: ProjectId) -> Option<DriverId> {
    drivers(s, project).first().map(|d| d.driver.id)
}

pub fn path(p: &str) -> &Path {
    Path::new(p)
}
