//! The daemon's settings: thresholds that are the same for every project. Each starts at a default
//! and changes only by an event in the daemon's log, so every client sees the values in force.
//!
//! The defaults are first guesses, to be measured and tuned against real providers in M17. How long
//! a driver stays warm is not among them: it is the prompt cache lifetime of the driver's model, on
//! its [`crate::harness::ModelOffer`].

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::decision::DecisionModel;
use crate::integration::{GitHubConnection, HomeboxConnection};
use crate::job::Interval;
use crate::money::Percent;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DaemonSettings {
    /// A driver is retired between tasks, or handed off mid-task, once its context passes this
    /// share of the model's window.
    pub retire_at_context: Percent,
    /// A driver is retired once it has carried this many tasks.
    pub retire_after_tasks: u32,
    /// A driver is retired once it has been alive this long.
    pub retire_after_age: Duration,
    /// A running driver silent for this long has failed. Longer while a command it started is
    /// still running, which is the driver's own business.
    pub stall_timeout: Duration,
    /// The deadline of a wait whose `wait` call named none.
    pub default_wait_deadline: Duration,
    /// The shortest repeat a builder or an agent may schedule.
    pub minimum_repeat: Interval,
    /// The most of a new model's context window its start pack may take.
    pub start_pack_share: Percent,
    /// The time limit on the last turn of a planned handoff. Past it, the handoff becomes a
    /// takeover.
    pub last_turn_limit: Duration,
    /// After this many turns in a row that end with the task neither reported nor waited on, the
    /// task is reported failed.
    pub turns_without_progress: u32,
    /// How long a harness and model are skipped after a failure that looks like the provider's.
    pub cool_off: Duration,
    /// The most terminal drivers that may run at once on one provider login, across every
    /// project. A subscription's limits assume one person's use.
    pub terminal_drivers_per_login: u32,
    /// Where a new project's code directory is made when it is set up, as
    /// `<projects root>/<its name as a slug>`. `None` is the daemon's own default, `~/x100`, which
    /// the daemon works out since this crate reads no environment.
    pub projects_root: Option<PathBuf>,
    /// Drivers run as the builder with no sandbox: they can read and change everything the
    /// builder can, and reach the network directly. Off by default. Without it, a daemon that
    /// cannot make a sandbox on its machine starts no driver at all.
    pub run_without_sandbox: bool,
    /// Directories or files a sandbox shows read-only besides the programs the daemon finds on
    /// its search path: a toolchain installed somewhere unusual.
    pub sandbox_programs: Vec<PathBuf>,
    /// The decision model that classifies actions no rule covers. `None`: nothing is classified,
    /// so an unruled read goes through and an unruled write waits for the builder.
    pub decision_model: Option<DecisionModel>,
    /// A decision model's answer whose confidence is below this share has its risk raised one
    /// level.
    pub decision_threshold: Percent,
    /// The Homebox x100 deploys to, once the builder connects one.
    pub homebox: Option<HomeboxConnection>,
    /// The GitHub account x100 creates repositories under and pushes to. Until it is connected,
    /// a project's repository stays on this machine.
    pub github: Option<GitHubConnection>,
}

const fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            retire_at_context: percent(70),
            retire_after_tasks: 10,
            retire_after_age: minutes(4 * 60),
            stall_timeout: minutes(5),
            default_wait_deadline: minutes(24 * 60),
            minimum_repeat: Interval::new(minutes(5)).expect("not zero"),
            start_pack_share: percent(50),
            last_turn_limit: minutes(3),
            turns_without_progress: 3,
            cool_off: minutes(10),
            terminal_drivers_per_login: 4,
            projects_root: None,
            run_without_sandbox: false,
            sandbox_programs: Vec::new(),
            decision_model: None,
            decision_threshold: percent(70),
            homebox: None,
            github: None,
        }
    }
}

fn percent(n: u8) -> Percent {
    Percent::new(n).expect("a percentage")
}

/// A change to one of the daemon's settings, as the daemon's log records it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonSetting {
    RetireAtContext(Percent),
    RetireAfterTasks(u32),
    RetireAfterAge(Duration),
    StallTimeout(Duration),
    DefaultWaitDeadline(Duration),
    MinimumRepeat(Interval),
    StartPackShare(Percent),
    LastTurnLimit(Duration),
    TurnsWithoutProgress(u32),
    CoolOff(Duration),
    TerminalDriversPerLogin(u32),
    ProjectsRoot(PathBuf),
    RunWithoutSandbox(bool),
    SandboxPrograms(Vec<PathBuf>),
    DecisionModel(Option<DecisionModel>),
    DecisionThreshold(Percent),
    Homebox(Option<HomeboxConnection>),
    GitHub(Option<GitHubConnection>),
}

impl DaemonSettings {
    pub fn set(&mut self, change: DaemonSetting) {
        use DaemonSetting as S;
        match change {
            S::RetireAtContext(v) => self.retire_at_context = v,
            S::RetireAfterTasks(v) => self.retire_after_tasks = v,
            S::RetireAfterAge(v) => self.retire_after_age = v,
            S::StallTimeout(v) => self.stall_timeout = v,
            S::DefaultWaitDeadline(v) => self.default_wait_deadline = v,
            S::MinimumRepeat(v) => self.minimum_repeat = v,
            S::StartPackShare(v) => self.start_pack_share = v,
            S::LastTurnLimit(v) => self.last_turn_limit = v,
            S::TurnsWithoutProgress(v) => self.turns_without_progress = v,
            S::CoolOff(v) => self.cool_off = v,
            S::TerminalDriversPerLogin(v) => self.terminal_drivers_per_login = v,
            S::ProjectsRoot(v) => self.projects_root = Some(v),
            S::RunWithoutSandbox(v) => self.run_without_sandbox = v,
            S::SandboxPrograms(v) => self.sandbox_programs = v,
            S::DecisionModel(v) => self.decision_model = v,
            S::DecisionThreshold(v) => self.decision_threshold = v,
            S::Homebox(v) => self.homebox = v,
            S::GitHub(v) => self.github = v,
        }
    }
}
