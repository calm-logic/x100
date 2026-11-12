//! Helpers for the daemon tools a scenario calls that need no flow of their own.

use std::time::Duration;

use crate::driver::{DriverEvent, Purpose, StartReason, StopReason};
use crate::effort::Effort;
use crate::event::ProjectEvent;
use crate::file::{ContentHash, FileRef};
use crate::id::{AgentId, DriverId, FileId, JobId, TrialId};
use crate::job::{Job, ProjectJob, Schedule};
use crate::memory::{SkillRef, SkillVersion};
use crate::money::Money;
use crate::party::{Actor, Party};
use crate::process::{Process, ProcessEnd, ProcessName};
use crate::spec::{SpecKind, SpecRevision, SpecStatus};
use crate::testing::{LogBuilder, commit, sonnet};
use crate::tool::{JobRequest, Proposal, ToolCall};
use crate::trial::{
    Arm, EvalItem, EvalSet, RunScore, Trial, TrialChange, TrialEvent, Verdict as TrialVerdict,
};

impl LogBuilder {
    pub fn write_spec(&mut self, driver: DriverId, kind: SpecKind, status: SpecStatus, body: &str) {
        let author = self.agent_of(driver);
        let revision = self.state().spec(kind).map_or(1, |s| s.spec.revision + 1);
        let call = ToolCall::WriteSpec {
            spec: kind,
            status,
            body: body.into(),
        };
        self.tool(driver, call, |b| {
            let spec = SpecRevision {
                kind,
                author,
                revision,
                status,
                body: body.into(),
            };
            b.push(Actor::Agent(author), ProjectEvent::SpecWritten(spec));
        });
    }

    pub fn add_file(&mut self, driver: DriverId, name: &str, size: u64) -> FileId {
        let agent = self.agent_of(driver);
        let hash = ContentHash(format!("sha256:{name}"));
        let id = self.id(FileId::new);
        let call = ToolCall::AddFile {
            name: name.into(),
            media_type: "image/png".into(),
            size,
            hash: hash.clone(),
        };
        self.tool(driver, call, |b| {
            let file = FileRef {
                id,
                name: name.into(),
                media_type: "image/png".into(),
                size,
                hash,
                added_by: Party::Agent(agent),
                attached_to: None,
            };
            b.push(Actor::Agent(agent), ProjectEvent::FileAdded(file));
        });
        id
    }

    pub fn schedule(&mut self, driver: DriverId, when: Schedule, does: JobRequest) -> JobId {
        let by = self.agent_of(driver);
        let id = self.id(JobId::new);
        let job = Job {
            id,
            when,
            does: match does.clone() {
                JobRequest::Wake { agent, note } => ProjectJob::Wake {
                    scheduled_by: by,
                    agent,
                    note,
                },
                JobRequest::Delegate {
                    to,
                    title,
                    ask,
                    effort,
                } => ProjectJob::Delegate {
                    scheduled_by: by,
                    to,
                    title,
                    ask,
                    effort,
                },
            },
        };
        self.tool(driver, ToolCall::Schedule { when, does }, |b| {
            b.push(Actor::Agent(by), ProjectEvent::JobScheduled(job));
        });
        id
    }

    pub fn process(&mut self, driver: DriverId, name: &str, command: Option<&str>) {
        let agent = self.agent_of(driver);
        let name = ProcessName(name.into());
        let (call, event) = match command {
            Some(command) => (
                ToolCall::StartProcess {
                    name: name.clone(),
                    command: command.into(),
                },
                ProjectEvent::ProcessStarted(Process {
                    agent,
                    name,
                    command: command.into(),
                }),
            ),
            None => (
                ToolCall::StopProcess { name: name.clone() },
                ProjectEvent::ProcessStopped {
                    agent,
                    name,
                    end: ProcessEnd::Stopped,
                },
            ),
        };
        self.tool(driver, call, |b| b.push(Actor::Daemon, event));
    }

    /// `propose_change` for a skill, then the trial in the lab: three runs of each arm on lab
    /// drivers of `agent`, scored, and committed or discarded.
    pub fn trial(
        &mut self,
        ceo_driver: DriverId,
        agent: AgentId,
        change: TrialChange,
        helps: bool,
    ) -> TrialId {
        let ceo = self.agent_of(ceo_driver);
        let id = self.id(TrialId::new);
        let eval = EvalSet {
            items: vec![EvalItem::Check(crate::trial::Check::Script {
                command: "npm test".into(),
            })],
            runs_per_arm: 3,
        };
        let change_kept = change.clone();
        let call = ToolCall::ProposeChange(Proposal::Trial {
            change: change.clone(),
            eval: eval.clone(),
        });
        self.tool(ceo_driver, call, |b| {
            let trial = Trial {
                id,
                change,
                eval,
                branch: format!("trial/{id}"),
            };
            b.push(Actor::Agent(ceo), ProjectEvent::TrialProposed(trial));
        });
        let trial_event = |b: &mut Self, event: TrialEvent| {
            b.push(Actor::Daemon, ProjectEvent::Trial { trial: id, event });
        };
        trial_event(self, TrialEvent::Started);
        for arm in [Arm::Baseline, Arm::Changed] {
            for run in 1..=3 {
                let purpose = Purpose::Lab {
                    trial: id,
                    arm,
                    run,
                };
                let why = StartReason::Wake;
                let d = self.start(agent, sonnet(), Effort::Medium, purpose, why);
                self.turn(d, 30_000, |_| {});
                self.driver_event(d, DriverEvent::Stopped(StopReason::LabRunEnded));
                let passed = if arm == Arm::Changed && helps { 3 } else { 2 };
                let score = RunScore {
                    arm,
                    run,
                    checks_passed: passed,
                    checks_total: 3,
                    tool_errors: 0,
                    turns: 1,
                    duration: Duration::from_secs(120),
                    cost: Money::from_micros(30_000),
                    judged: None,
                };
                trial_event(self, TrialEvent::RunFinished(score));
            }
        }
        if helps {
            trial_event(self, TrialEvent::Scored(TrialVerdict::Supports));
            let commit = commit("trial");
            trial_event(
                self,
                TrialEvent::Committed {
                    commit: commit.clone(),
                },
            );
            let event = match change_kept {
                TrialChange::EditSkill { skill, .. } | TrialChange::NewSkill { skill, .. } => {
                    let version = self.next_version(&skill);
                    ProjectEvent::SkillCommitted {
                        skill,
                        version,
                        commit,
                        trial: id,
                    }
                }
                TrialChange::CompactMemory { agent, .. } => ProjectEvent::MemoryCompacted {
                    agent,
                    commit,
                    trial: id,
                },
            };
            self.push(Actor::Daemon, event);
        } else {
            trial_event(self, TrialEvent::Scored(TrialVerdict::DoesNotSupport));
            trial_event(self, TrialEvent::Discarded);
        }
        id
    }

    /// One past the highest version of `skill` loaded or committed so far.
    fn next_version(&self, skill: &SkillRef) -> SkillVersion {
        let s = self.state();
        let committed = s
            .skills()
            .iter()
            .filter(|r| r.skill == *skill)
            .map(|r| r.version);
        let loaded = s
            .agents()
            .flat_map(|a| a.skills.iter())
            .filter(|l| l.skill == *skill)
            .map(|l| l.version);
        let highest = committed.chain(loaded).max().map_or(0, |v| v.0);
        SkillVersion(highest + 1)
    }

    pub fn load_skill(&mut self, driver: DriverId, skill: SkillRef, version: u32) {
        let agent = self.agent_of(driver);
        self.tool(
            driver,
            ToolCall::LoadSkill {
                skill: skill.clone(),
            },
            |b| {
                let event = ProjectEvent::SkillLoaded {
                    agent,
                    skill,
                    version: SkillVersion(version),
                };
                b.push(Actor::Daemon, event);
            },
        );
    }
}
