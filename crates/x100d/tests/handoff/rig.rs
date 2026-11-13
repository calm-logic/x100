//! A daemon as a child process on the fake clock, with a seed, and the fake harness posing as two
//! providers: `fake-anthropic`, the cheaper, which every first driver runs on, and `fake-openai`.
//! Both play the same turns, so a driver that takes over finds its turn whichever it is.
//!
//! Each test names its scenario with one word. The builder's message `SCENARIO <word>` has the
//! CEO set the project up with the brief `<word>: ...`, which opens the CEO's task; the scenario's
//! own turns match the task's input, `A task for you: <word>`, and a driver that takes over tells
//! itself apart by its start pack, which says "You take over".
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::*;
use x100_client::Client;
use x100_core::driver::DriverState;
use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::{DriverRecord, ProjectState};
use x100_core::id::{AgentId, DriverId, ProjectId, TaskId};
use x100_core::settings::DaemonSetting;
use x100_core::time::Timestamp;
use x100_core::transcript::{Entry, Item};
use x100_proto::{
    CreateProject, LogId, LogPage, Page, ReadLog, ReadStartPack, ReadTranscript, SetClock,
    SetDaemonSetting,
};

/// Where the fake clock starts.
pub const START: Timestamp = Timestamp::from_millis(1_794_000_000_000);

/// The cheaper provider, whose drivers start first, and the dearer one.
pub const CHEAP: &str = "fake-anthropic";
pub const DEAR: &str = "fake-openai";

fn harness(name: &str, model: &str, input: f64, output: f64, turns: &str) -> String {
    format!(
        r#"
[harness]
name = "{name}"

[[model]]
model = "{model}"
context_window = 200000
efforts = ["low", "medium", "high", "max"]
price = {{ input = {input}, output = {output} }}

{turns}

# Anything else, such as a worker's report reaching the CEO, is noted.
[[turn]]
repeat = true
steps = [{{ text = "noted" }}]
"#
    )
}

/// The set-up turn of a scenario: the brief is `<word>: the work`.
pub fn set_up(word: &str) -> String {
    format!(
        r#"
[[turn]]
when = {{ input_contains = "SCENARIO {word}" }}
repeat = true
steps = [
  {{ call = {{ set_up_project = {{ name = "Handoff", intent = "Hand work between drivers.", brief = "{word}: the work." }} }} }},
]
"#
    )
}

pub struct Rig {
    pub env: Env,
    pub daemon: DaemonProc,
    pub client: Client,
    pub project: ProjectId,
    pub ceo: AgentId,
    pub dir: PathBuf,
}

pub fn args(env: &Env) -> Vec<std::ffi::OsString> {
    let mut args = env.daemon_args(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")));
    args.extend(["--fake-clock".into(), START.as_millis().to_string().into()]);
    args.extend(["--seed".into(), "7".into()]);
    args
}

impl Rig {
    /// A project on a folder of its own, whose drivers play `turns` on either provider.
    pub async fn start(name: &str, turns: &str) -> Rig {
        let env = Env::new();
        std::fs::remove_file(env.scripts.join("fake.toml")).unwrap();
        let write =
            |file: &str, text: String| std::fs::write(env.scripts.join(file), text).unwrap();
        write("anthropic.toml", harness(CHEAP, "fake-a", 1.0, 5.0, turns));
        write("openai.toml", harness(DEAR, "fake-o", 2.0, 8.0, turns));
        let daemon = DaemonProc::spawn(args(&env));
        let client = env.client().await;
        // Room for turns that end mid-task, which a planned handoff follows.
        client
            .request(SetDaemonSetting {
                setting: DaemonSetting::TurnsWithoutProgress(5),
            })
            .await
            .unwrap();
        std::fs::create_dir_all(env.project_dir(name)).unwrap();
        let project = client
            .request(CreateProject {
                dir: Some(env.project_dir(name)),
                settings: None,
            })
            .await
            .unwrap()
            .project;
        let ceo = client
            .wait_for(WAIT, |s| Some(s.projects.get(&project)?.project()?.ceo))
            .await
            .unwrap();
        let dir = PathBuf::from(env.project_dir(name));
        Rig {
            env,
            daemon,
            client,
            project,
            ceo,
            dir,
        }
    }

    /// The builder's message that starts `word`, and the CEO's task it opens.
    pub async fn brief(&self, word: &str) -> TaskId {
        self.client
            .send_chat(self.project, &format!("SCENARIO {word}"), vec![], None)
            .await
            .unwrap();
        self.until("the CEO's task is open", |p| {
            p.tasks()
                .find(|t| t.task.title.starts_with(word))
                .map(|t| t.task.id)
        })
        .await
    }

    pub fn state(&self) -> ProjectState {
        self.client.project(self.project).expect("the project")
    }

    pub async fn until<T>(&self, what: &str, f: impl Fn(&ProjectState) -> Option<T>) -> T {
        let project = self.project;
        match self
            .client
            .wait_for(WAIT, |s| f(s.projects.get(&project)?))
            .await
        {
            Ok(v) => v,
            Err(_) => {
                let drivers: Vec<_> = self
                    .state()
                    .drivers()
                    .map(|d| (d.driver.id, d.state.clone()))
                    .collect();
                panic!("timed out waiting until {what}; drivers: {drivers:?}")
            }
        }
    }

    /// The agent's drivers, oldest first.
    pub fn drivers_of(&self, agent: AgentId) -> Vec<DriverRecord> {
        let s = self.state();
        s.agent(agent)
            .map(|a| {
                a.drivers
                    .iter()
                    .filter_map(|d| s.driver(*d).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The name of the provider a driver runs on.
    pub fn provider(&self, driver: DriverId) -> String {
        let harness = self.state().driver(driver).unwrap().driver.harness;
        self.client
            .with_state(|s| s.daemon.harness(harness).map(|h| h.kind.clone()))
            .map(|kind| match kind {
                x100_core::harness::HarnessKind::Custom { name } => name,
                other => format!("{other:?}"),
            })
            .expect("the harness is registered")
    }

    /// Waits until the agent's `n`th driver (from 1) is in a state `f` accepts.
    pub async fn nth_driver(
        &self,
        agent: AgentId,
        n: usize,
        f: impl Fn(&DriverState) -> bool,
    ) -> DriverId {
        self.until(&format!("driver {n} of {agent}"), |p| {
            let d = *p.agent(agent)?.drivers.get(n - 1)?;
            f(&p.driver(d)?.state).then_some(d)
        })
        .await
    }

    /// Waits until a driver of `agent` begins a tool call named `name`, and returns that driver.
    pub async fn begins(&self, agent: AgentId, name: &str) -> DriverId {
        self.recorded(
            agent,
            &format!("{agent} begins {name}"),
            |e| matches!(&e.item, Item::ToolCall { name: n, .. } if n == name),
        )
        .await
    }

    /// Waits until a driver of `agent` writes `item` in its transcript, and returns that driver.
    pub async fn wrote(&self, agent: AgentId, item: &Item) -> DriverId {
        self.recorded(agent, &format!("{agent} writes {item:?}"), |e| {
            e.item == *item
        })
        .await
    }

    async fn recorded(&self, agent: AgentId, what: &str, f: impl Fn(&Entry) -> bool) -> DriverId {
        let until = std::time::Instant::now() + WAIT;
        loop {
            if let Some(e) = self.transcript(agent).await.iter().find(|e| f(e)) {
                return e.driver;
            }
            assert!(std::time::Instant::now() < until, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The agent's drivers from `first` on, oldest first: those its task had, after the driver
    /// that set the project up.
    pub fn drivers_since(&self, agent: AgentId, first: DriverId) -> Vec<DriverRecord> {
        let all = self.drivers_of(agent);
        let at = all
            .iter()
            .position(|d| d.driver.id == first)
            .expect("the driver is the agent's");
        all[at..].to_vec()
    }

    /// SIGKILL to every process of the driver's, as an outside cause would kill it.
    pub fn kill(&self, driver: DriverId) {
        let pids: Vec<u32> = self
            .env
            .recorded_pids()
            .into_iter()
            .filter(|(owner, _)| *owner == driver.to_string())
            .map(|(_, pid)| pid)
            .collect();
        assert!(!pids.is_empty(), "{driver} has a process on record");
        for pid in pids {
            let group = nix::unistd::Pid::from_raw(pid as i32);
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
        }
    }

    pub async fn transcript(&self, agent: AgentId) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .client
                .request(ReadTranscript {
                    project: self.project,
                    agent,
                    after,
                    limit: 500,
                })
                .await
                .unwrap();
            let Some(last) = page.entries.last().map(|e| e.position) else {
                return out;
            };
            after = Some(last);
            out.extend(page.entries.into_iter().map(|e| e.entry));
        }
    }

    pub async fn start_pack(&self, driver: DriverId) -> String {
        self.client
            .request(ReadStartPack {
                project: self.project,
                driver,
            })
            .await
            .expect("a start pack")
            .text
    }

    pub async fn log(&self) -> Vec<Envelope<ProjectEvent>> {
        let mut out = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .client
                .request(ReadLog {
                    log: LogId::Project(self.project),
                    page: Page::OldestFirst { after, limit: 2000 },
                })
                .await
                .unwrap();
            let LogPage::Project { events } = page else {
                panic!("a project's log")
            };
            let Some(last) = events.last().map(|e| e.seq) else {
                return out;
            };
            after = Some(last);
            out.extend(events);
        }
    }

    pub async fn clock_to(&self, at: Timestamp) {
        self.client.request(SetClock { at }).await.unwrap();
    }

    /// The agent's worktree, where its drivers work.
    pub fn worktree(&self, agent: AgentId) -> PathBuf {
        self.dir.join(".x100/worktrees").join(agent.to_string())
    }

    /// SIGKILL to the daemon, then the daemon again on the same data directory.
    pub async fn kill_and_restart_daemon(&mut self) {
        self.daemon.kill9();
        self.daemon = DaemonProc::spawn(args(&self.env));
        self.client = self.env.client().await;
        let project = self.project;
        self.client
            .wait_for(WAIT, |s| s.projects.get(&project).map(|_| ()))
            .await
            .expect("the project is back");
    }

    /// Stops the daemon, and checks no driver's process outlived it.
    pub async fn finish(mut self) {
        let pids: Vec<u32> = self
            .env
            .recorded_pids()
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        self.daemon.terminate();
        gone(&pids).await;
    }
}
