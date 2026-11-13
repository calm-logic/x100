//! A daemon as a child process on the fake harness, with a project that has done one piece of
//! work: the builder's brief, "Write the greeting.", which the CEO did and reported, so it ended
//! accepted and can be replayed. The CEO remembered three notes on the way, one of them wrong.
//!
//! The script's lab turns come first. A lab driver's pack begins "LAB RUN", and a skill on trial
//! is in it in full, so a scripted driver can do better with a change than without it: given the
//! skill that says GREETING-HELPS it writes what the check looks for, and otherwise it does not.
//! The CEO's reflection turn is the test's own. The daemon runs on the fake clock, with a seed.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::*;
use x100_client::Client;
use x100_core::driver::Purpose;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId, ProjectId, TaskId};
use x100_core::ledger::{Charge, Payer};
use x100_core::task::TaskState;
use x100_core::trial::TrialState;
use x100_proto::{LogId, LogPage, Page, ReadLog, ReadStartPack};

/// Trials run several lab drivers each.
pub const LONG: Duration = Duration::from_secs(180);

/// What the lab drivers and the CEO do before reflecting. `reflection` is the steps of the CEO's
/// reflection turn, `lab` any further lab turns, put first.
pub fn script(reflection: &str, lab: &str) -> String {
    format!(
        r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = {{ input = 1.0, output = 5.0 }}

{lab}

# A lab driver replaying the brief with the helpful skill in its pack writes what the check wants.
[[turn]]
when = {{ start_pack_contains = "GREETING-HELPS", input_contains = "Write the greeting" }}
repeat = true
steps = [
  {{ write = "greeting.txt", content = "hello" }},
  {{ usage = {{ input = 10, output = 5, cost_micros = 2000 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "wrote hello" }} }} }},
]

# Any other lab driver replaying it writes something else.
[[turn]]
when = {{ start_pack_contains = "LAB RUN", input_contains = "Write the greeting" }}
repeat = true
steps = [
  {{ write = "greeting.txt", content = "hi" }},
  {{ usage = {{ input = 10, output = 5, cost_micros = 2000 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "wrote hi" }} }} }},
]

# A probe is answered from the note `deploys` as the arm's branch holds it.
[[turn]]
when = {{ start_pack_contains = "LAB RUN", input_contains = "Question:" }}
repeat = true
steps = [
  {{ call = {{ read_note = {{ note = "deploys" }} }}, save = "n" }},
  {{ usage = {{ input = 10, output = 5, cost_micros = 1000 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "{{{{n}}}}" }} }} }},
]

[[turn]]
when = {{ input_contains = "BRIEF" }}
repeat = true
steps = [
  {{ call = {{ set_up_project = {{ name = "Lab", intent = "Try the lab.", brief = "Write the greeting." }} }} }},
]

[[turn]]
when = {{ input_contains = "A task for you: Write the greeting" }}
repeat = true
steps = [
  {{ write = "greeting.txt", content = "hello" }},
  {{ call = {{ remember = {{ note = "deploys", body = "Deploys go by hand.", sources = [1] }} }} }},
  {{ call = {{ remember = {{ note = "host", body = "The host is box-7, in the north rack of the second room on the left.", sources = [1] }} }} }},
  {{ call = {{ remember = {{ note = "lunch", body = "The builder eats lunch at noon every day, at the cafe on the corner by the station.", sources = [1] }} }} }},
  {{ usage = {{ input = 100, output = 20, cost_micros = 1234 }} }},
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "Wrote the greeting." }} }} }},
]

[[turn]]
when = {{ input_contains = "Reflect on the last piece of work" }}
repeat = true
steps = [
  {{ call = {{ read_log = {{ from = 1 }} }}, save = "log" }},
  {{ call = {{ read_memories = {{ agent = "{{{{pack:Your id}}}}" }} }}, save = "mem" }},
{reflection}
  {{ call = {{ report = {{ task = "{{{{input:Task}}}}", outcome = "completed", summary = "Reflected." }} }} }},
]

# Anything else, such as a trial's verdict reaching the CEO, is noted and costs nothing.
[[turn]]
repeat = true
steps = [{{ text = "noted" }}]
"##
    )
}

/// A skill as reflection proposes it, with its front matter.
pub fn skill_body(name: &str, line: &str) -> String {
    format!("---\\nname: {name}\\ndescription: How to {name}.\\n---\\n\\n{line}\\n")
}

/// The daemon's arguments: the fake clock, so a run's time is the same in each arm, and a seed,
/// so each run of the test makes the same choices.
fn args(env: &Env) -> Vec<std::ffi::OsString> {
    let mut args = env.daemon_args(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")));
    args.extend(["--fake-clock".into(), "1794000000000".into()]);
    args.extend(["--seed".into(), "7".into()]);
    args
}

pub struct Rig {
    pub env: Env,
    pub daemon: DaemonProc,
    pub client: Client,
    pub project: ProjectId,
    pub ceo: AgentId,
    pub dir: PathBuf,
    /// The brief, done and accepted.
    pub brief: TaskId,
}

impl Rig {
    /// A project whose CEO has done the brief. `extra` is put in the daemon's environment.
    pub async fn start(name: &str, script: &str, extra: &[(&str, String)]) -> Rig {
        let env = Env::new();
        std::fs::write(env.scripts.join("fake.toml"), script).unwrap();
        let daemon = DaemonProc::spawn_adding(args(&env), extra);
        let client = env.client().await;
        let project = create_project(&client, &env, name).await;
        let ceo = client.project(project).unwrap().project().unwrap().ceo;
        client
            .send_chat(project, "BRIEF", vec![], None)
            .await
            .unwrap();
        let brief = client
            .wait_for(WAIT, |s| {
                s.projects
                    .get(&project)?
                    .tasks()
                    .find_map(|t| matches!(t.state, TaskState::Done(..)).then_some(t.task.id))
            })
            .await
            .expect("the brief is done and accepted");
        let dir = PathBuf::from(env.project_dir(name));
        let rig = Rig {
            env,
            daemon,
            client,
            project,
            ceo,
            dir,
            brief,
        };
        rig.idle().await;
        rig
    }

    /// Stops the daemon cleanly and starts it again on the same data, with a new client.
    pub async fn restart(&mut self) {
        self.daemon.terminate();
        self.daemon = DaemonProc::spawn(args(&self.env));
        self.client = self.env.client().await;
        let project = self.project;
        self.client
            .wait_for(WAIT, |s| s.projects.get(&project).map(|_| ()))
            .await
            .expect("the project is back");
    }

    pub fn state(&self) -> ProjectState {
        self.client.project(self.project).expect("the project")
    }

    /// Waits until the CEO has nothing waiting and no lab driver is live.
    pub async fn idle(&self) {
        self.client
            .wait_for(LONG, |s| {
                let p = s.projects.get(&self.project)?;
                let ceo = p.agent(self.ceo)?;
                let busy = p.drivers().any(|d| {
                    d.state.is_live()
                        && (d.state == x100_core::driver::DriverState::Running
                            || d.driver.purpose != Purpose::Work)
                });
                (ceo.inbox.is_empty() && !busy).then_some(())
            })
            .await
            .expect("the project goes quiet");
    }

    pub async fn set_trials(&self, on: bool) {
        self.client
            .set_trial_before_commit(self.project, on)
            .await
            .unwrap();
        self.client
            .wait_for(WAIT, |s| {
                let p = s.projects.get(&self.project)?.project()?;
                (p.settings.trial_before_commit == on).then_some(())
            })
            .await
            .unwrap();
    }

    /// Reflect now, and waits until the reflection is reported, every trial has ended and the
    /// project is quiet.
    pub async fn reflect(&self) -> TaskId {
        let task = self
            .client
            .reflect(self.project)
            .await
            .expect("reflect now");
        self.client
            .wait_for(LONG, |s| {
                let p = s.projects.get(&self.project)?;
                let done = matches!(p.task(task)?.state, TaskState::Done(..));
                let settled = p
                    .trials()
                    .all(|t| !matches!(t.state, TrialState::Proposed | TrialState::Running));
                (done && settled).then_some(())
            })
            .await
            .expect("the reflection is done and its trials have ended");
        self.idle().await;
        task
    }

    pub async fn events(&self) -> Vec<Envelope<ProjectEvent>> {
        let mut all = Vec::new();
        let mut after: Option<Seq> = None;
        loop {
            let page = self
                .client
                .request(ReadLog {
                    log: LogId::Project(self.project),
                    page: Page::OldestFirst { after, limit: 1000 },
                })
                .await
                .unwrap();
            let LogPage::Project { events } = page else {
                panic!("a project's log")
            };
            let Some(last) = events.last().map(|e| e.seq) else {
                return all;
            };
            after = Some(last);
            all.extend(events);
        }
    }

    pub async fn pack(&self, driver: DriverId) -> String {
        self.client
            .request(ReadStartPack {
                project: self.project,
                driver,
            })
            .await
            .expect("the driver's start pack")
            .text
    }

    /// The lab drivers the log records.
    pub fn lab_drivers(&self) -> Vec<x100_core::fold::DriverRecord> {
        self.state()
            .drivers()
            .filter(|d| d.driver.purpose != Purpose::Work)
            .cloned()
            .collect()
    }
}

/// Every charge in the log, in order, as the bytes of its record.
pub fn ledger(events: &[Envelope<ProjectEvent>], lab: bool) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::Charged(c @ Charge { payer, .. }) if (*payer == Payer::Lab) == lab => {
                Some(serde_json::to_string(&(e.seq, c)).unwrap())
            }
            _ => None,
        })
        .collect()
}

/// Every file under `dir` but the memory and skill repositories and git's own records, by path,
/// with its bytes. These are what a trial must never touch.
pub fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if rel == ".git" || rel == ".x100/agents" || rel == ".x100/skills" {
                continue;
            }
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                walk(root, &path, out);
            } else if kind.is_symlink() {
                out.insert(
                    rel,
                    std::fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_encoded_bytes(),
                );
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// The project repository's refs, but each agent's checkpoint, which every turn moves.
pub fn refs(dir: &Path) -> String {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["for-each-ref", "--format=%(refname) %(objectname)"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| !l.starts_with("refs/x100/checkpoints/"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}
