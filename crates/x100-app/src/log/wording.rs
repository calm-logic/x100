//! How an event reads in the Log, in plain words. Every match here names every variant and has no
//! catch-all arm, so an event added to core without wording here does not compile.

use std::collections::HashMap;

use x100_core::action::Decision;
use x100_core::agent::{AgentEvent, Position};
use x100_core::credential::{UseOutcome, Via};
use x100_core::driver::{Purpose, StartReason};
use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{ActionId, AgentId, DriverId, FileId, JobId, TaskId, TrialId};
use x100_core::inbox::InboxItem;
use x100_core::job::{JobEvent, ProjectJob, ProjectRoutine, Schedule};
use x100_core::ledger::{BudgetReached, ChargeSource, Payer};
use x100_core::party::{Actor, Party};
use x100_core::process::ProcessEnd;
use x100_core::project::SettingChange;
use x100_core::task::{Opening, Outcome, TaskEvent, Verdict};
use x100_core::trial::{TrialChange, TrialEvent, Verdict as TrialVerdict};
use x100_core::wait::{WaitEnd, WaitOn};

mod actions;
mod drivers;

pub use actions::{kind, request};

use crate::words;
use actions::action_event;
pub(crate) use drivers::unfit;
use drivers::{driver_event, handoff, incompatible, one_off};

/// One line of the Log: who did it, and what, as a sentence without a full stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub who: String,
    pub text: String,
}

/// What the wording looks names up in. Anything gone from the state falls back to a plain noun.
pub struct Names<'a> {
    pub project: &'a ProjectState,
    pub daemon: &'a DaemonState,
    /// Names of files that have since been removed, from the events that added them.
    pub files: &'a HashMap<FileId, String>,
}

impl Names<'_> {
    fn agent(&self, id: AgentId) -> String {
        self.project
            .agent(id)
            .map_or_else(|| "an agent".into(), |a| a.role.clone())
    }

    fn task(&self, id: TaskId) -> String {
        self.project.task(id).map_or_else(
            || "a task".into(),
            |t| format!("\u{201c}{}\u{201d}", t.task.title),
        )
    }

    fn file(&self, id: FileId) -> String {
        self.project
            .file(id)
            .map(|f| f.file.name.clone())
            .or_else(|| self.files.get(&id).cloned())
            .unwrap_or_else(|| "a file".into())
    }

    fn driver_agent(&self, id: DriverId) -> String {
        self.project
            .driver(id)
            .map_or_else(|| "an agent".into(), |d| self.agent(d.driver.agent))
    }

    fn driver(&self, id: DriverId) -> String {
        self.project.driver(id).map_or_else(
            || "a driver".into(),
            |d| {
                format!(
                    "{} \u{b7} {}",
                    words::harness(self.daemon, d.driver.harness),
                    d.driver.model.0
                )
            },
        )
    }

    fn payer(&self, payer: Payer) -> String {
        match payer {
            Payer::Project => "the project's budget".into(),
            Payer::Lead(lead) => format!("{}'s budget", self.agent(lead)),
            Payer::Lab => "the lab's budget".into(),
        }
    }

    fn party(&self, party: Party) -> String {
        match party {
            Party::Builder => "the builder".into(),
            Party::Agent(a) => self.agent(a),
        }
    }

    fn job(&self, id: JobId) -> String {
        match self.project.job(id).map(|j| &j.job.does) {
            Some(does) => job(self, does),
            None => "a job".into(),
        }
    }

    fn action(&self, id: ActionId) -> String {
        self.project
            .action(id)
            .map_or_else(|| "an action".into(), |a| request(self, &a.action.request))
    }
}

/// The line for one event of a project's log.
pub fn line(e: &Envelope<ProjectEvent>, n: &Names) -> Line {
    let who = match e.by {
        Actor::Builder => "You".to_owned(),
        Actor::Agent(a) => n.agent(a),
        Actor::Daemon => "x100".to_owned(),
    };
    Line {
        who,
        text: text(&e.event, n),
    }
}

fn text(event: &ProjectEvent, n: &Names) -> String {
    use ProjectEvent as P;
    match event {
        P::Created { .. } => "Started a new project, not set up yet".into(),
        P::CodeDirSet { dir } => format!("Keeps the project's code in {}", dir.display()),
        P::SetUp(i) => format!("Set the project up as {}: {}", quote(&i.name), i.intent),
        P::Renamed(i) => format!("Renamed the project {}: {}", quote(&i.name), i.intent),
        P::SettingChanged(change) => setting(n, change),
        P::PreviewSet { url } => format!("Set the preview address to {url}"),
        P::AgentAdded(a) => match &a.position {
            Position::Ceo { .. } => "Started the CEO".into(),
            Position::Lead { effort, budget } => format!(
                "Brought in a {} lead, effort {} to {}, budget {}",
                a.role,
                words::effort(effort.min()),
                words::effort(effort.max()),
                words::money(*budget)
            ),
            Position::Worker { parent, .. } => {
                format!("Started a {} agent under {}", a.role, n.agent(*parent))
            }
        },
        P::Agent { agent, event } => agent_event(n, *agent, event),
        P::DriverStarted(d) => {
            let what = format!(
                "{} \u{b7} {} at {} effort for {}",
                words::harness(n.daemon, d.harness),
                d.model.0,
                words::effort(d.effort).to_lowercase(),
                n.agent(d.agent)
            );
            let why = match &d.why {
                StartReason::NewTask { task } => format!(" to work on {}", n.task(*task)),
                StartReason::Wake => " to wake it".into(),
                StartReason::Handoff(h) => format!(", {}", handoff(n, h)),
            };
            match d.purpose {
                Purpose::Work => format!("Chose {what}{why}"),
                Purpose::Lab { .. } => format!("Started a lab run on {what}"),
            }
        }
        P::Driver { driver, event } => driver_event(n, *driver, event),
        P::ProcessStarted(p) => format!(
            "Started {} for {}: {}",
            p.name.0,
            n.agent(p.agent),
            p.command
        ),
        P::ProcessStopped { agent, name, end } => format!(
            "{}'s process {} {}",
            n.agent(*agent),
            name.0,
            match end {
                ProcessEnd::Stopped => "was stopped".to_owned(),
                ProcessEnd::Exited { code: Some(0) } => "finished".to_owned(),
                ProcessEnd::Exited { code: Some(c) } => format!("exited with code {c}"),
                ProcessEnd::Exited { code: None } => "was ended by a signal".to_owned(),
                ProcessEnd::SandboxDestroyed => "ended with its sandbox".to_owned(),
            }
        ),
        P::Checkpointed { agent, .. } => {
            format!("Saved a checkpoint of {}'s work", n.agent(*agent))
        }
        P::BranchRefreshed { agent, from, .. } => format!(
            "Brought {}'s branch up to date with {}'s work",
            n.agent(*agent),
            n.party(*from)
        ),
        P::OneOffCalled(c) => one_off(n, c),
        P::Charged(c) => format!(
            "Charged {} to {} for {}",
            words::money(c.cost),
            n.payer(c.payer),
            match c.source {
                ChargeSource::Turn { agent, .. } => format!("a turn of {}", n.agent(agent)),
                ChargeSource::OneOff { .. } => "a call to the decision model".to_owned(),
            }
        ),
        P::BudgetReached(b) => match b {
            BudgetReached::Project => "The project reached its budget".into(),
            BudgetReached::Lead(lead) => format!("{} reached its budget", n.agent(*lead)),
            BudgetReached::Lab => "The lab reached its cap".into(),
        },
        P::TaskOpened { task, opening } => match opening {
            Opening::Raised => format!("Raised the task {}", quote(&task.title)),
            Opening::Delegated(a) => format!(
                "Gave {} the task {}",
                n.agent(a.assignee),
                quote(&task.title)
            ),
        },
        P::Task { task, event } => task_event(n, *task, event),
        P::MessageSent(m) => match m.route {
            x100_core::message::Route::BuilderToCeo => {
                format!("Wrote to the CEO: {}", snippet(&m.body))
            }
            x100_core::message::Route::CeoToBuilder => {
                format!("Wrote to the builder: {}", snippet(&m.body))
            }
            x100_core::message::Route::DaemonToBuilder => {
                format!("Told the builder: {}", snippet(&m.body))
            }
            x100_core::message::Route::Between { to, .. } => {
                format!("Wrote to {}: {}", n.agent(to), snippet(&m.body))
            }
        },
        P::JobScheduled(j) => format!(
            "Scheduled {} {}",
            job(n, &j.does),
            match j.when {
                Schedule::Once { at } => format!("for {}", words::moment(at)),
                Schedule::Every { first, every } => format!(
                    "from {}, every {}",
                    words::moment(first),
                    duration(every.get())
                ),
            }
        ),
        P::Job { job, event } => match event {
            JobEvent::Fired { .. } => format!("Fired {}", n.job(*job)),
            JobEvent::Cancelled { by } => format!(
                "Cancelled {}{}",
                n.job(*job),
                match by {
                    Actor::Builder => " at the builder's request".to_owned(),
                    Actor::Agent(a) => format!(" at {}'s request", n.agent(*a)),
                    Actor::Daemon => String::new(),
                }
            ),
        },
        P::ActionProposed(a) => format!("{} wants to {}", n.agent(a.agent), request(n, &a.request)),
        P::Action { action, event } => action_event(n, *action, event),
        P::SpecWritten(s) => format!(
            "{} the {} (revision {}, {})",
            if s.revision > 1 { "Revised" } else { "Wrote" },
            words::spec_name(s.kind).to_lowercase(),
            s.revision,
            match s.status {
                x100_core::spec::SpecStatus::Draft => "Draft",
                x100_core::spec::SpecStatus::Approved => "Approved",
            }
        ),
        P::FileAdded(f) => format!(
            "Added {} ({}, {})",
            f.name,
            words::kind(&f.media_type).to_lowercase(),
            words::size(f.size)
        ),
        P::FileRemoved { file } => format!("Removed {} to save space", n.file(*file)),
        P::MemoryWritten { agent, note, .. } => {
            format!("Wrote the memory note {} for {}", note.0, n.agent(*agent))
        }
        P::SkillLoaded {
            agent,
            skill,
            version,
        } => format!(
            "{} loaded the skill {} (version {})",
            n.agent(*agent),
            skill.name.0,
            version.0
        ),
        P::TrialProposed(t) => format!("Proposed a trial: {}", trial_change(n, &t.change)),
        P::Trial { trial, event } => trial_event(n, *trial, event),
        P::SkillCommitted { skill, version, .. } => format!(
            "Committed version {} of the skill {}",
            version.0, skill.name.0
        ),
        P::MemoryCompacted { agent, .. } => format!("Compacted {}'s memory", n.agent(*agent)),
        P::CredentialUsed(u) => {
            let name = n
                .daemon
                .credentials()
                .find(|c| c.credential.reference == u.credential)
                .map_or_else(|| "a credential".into(), |c| c.credential.name.clone());
            let via = match &u.via {
                Via::Proxy { host } => format!("for a request to {host}"),
                Via::Wrapper { command } => format!("for the command {command}"),
            };
            match &u.outcome {
                UseOutcome::Injected => format!("Used {name} {via} for {}", n.agent(u.agent)),
                UseOutcome::Refused(why) => {
                    format!("Refused {name} {via} for {}: {why:?}", n.agent(u.agent))
                }
            }
        }
    }
}

fn setting(n: &Names, change: &SettingChange) -> String {
    match change {
        SettingChange::AskMeAbout(level) => {
            format!("Changed Ask me about to {}", words::ask_me_about(*level))
        }
        SettingChange::Budget(total) => {
            format!("Set the project budget to {}", words::money(*total))
        }
        SettingChange::LabShare(share) => {
            format!("Set the lab's share of the budget to {}%", share.value())
        }
        SettingChange::Allow { what, allowed } => format!(
            "{} {} \u{b7} {}",
            if *allowed {
                "Allowed"
            } else {
                "Stopped allowing"
            },
            words::harness(n.daemon, what.harness),
            what.model.0
        ),
        SettingChange::Homebox(Some(link)) => format!(
            "Linked the project to the Homebox project {} ({}), {}",
            link.project_name, link.repo_full_name, link.environment
        ),
        SettingChange::Homebox(None) => "Unlinked the project from Homebox".into(),
        SettingChange::Repository(full) => {
            format!("Published the project to the GitHub repository {full}")
        }
    }
}

fn agent_event(n: &Names, agent: AgentId, event: &AgentEvent) -> String {
    let who = n.agent(agent);
    match event {
        AgentEvent::Delivered { what, .. } => format!("{who} was given {}", inbox(n, what)),
        AgentEvent::WaitOpened(w) => format!(
            "{who} began waiting for {} until {}",
            wait_on(n, &w.on),
            words::moment(w.deadline)
        ),
        AgentEvent::WaitEnded { end, .. } => format!(
            "{who}'s wait {}",
            match end {
                WaitEnd::Satisfied => "was satisfied",
                WaitEnd::Expired => "ran out",
                WaitEnd::CannotHappen => "can no longer be satisfied",
            }
        ),
        AgentEvent::NoDriver(why) => format!(
            "No driver fits {who}: {}",
            why.iter()
                .map(|i| incompatible(n, i))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        AgentEvent::DriverFound => format!("Found a driver for {who} again"),
        AgentEvent::StillWaiting => format!("{who} is still waiting"),
        AgentEvent::WentIdle => format!("{who} went idle"),
        AgentEvent::Retired => format!("Retired {who}"),
        AgentEvent::EffortSet(r) => format!(
            "Set {who}'s effort to {} to {}",
            words::effort(r.min()),
            words::effort(r.max())
        ),
        AgentEvent::BudgetSet(m) => format!("Set {who}'s budget to {}", words::money(*m)),
        AgentEvent::Moved { under } => format!("Moved {who} under {}", n.agent(*under)),
        AgentEvent::Promoted { effort, budget } => format!(
            "Made {who} a lead, effort {} to {}, budget {}",
            words::effort(effort.min()),
            words::effort(effort.max()),
            words::money(*budget)
        ),
    }
}

fn inbox(n: &Names, item: &InboxItem) -> String {
    match item {
        InboxItem::Task { task } => format!("the task {}", n.task(*task)),
        InboxItem::Message { .. } => "a message".into(),
        InboxItem::Report { task } => format!("the report on {}", n.task(*task)),
        InboxItem::Decision {
            action, decision, ..
        } => format!(
            "the builder's {} of {}",
            match decision {
                Decision::Approved => "approval",
                Decision::Denied => "refusal",
            },
            n.action(*action)
        ),
        InboxItem::Finished { action, .. } => format!("the result of {}", n.action(*action)),
        InboxItem::Job { note, .. } => format!("a reminder: {}", snippet(note)),
        InboxItem::WaitEnded { .. } => "word that a wait ended".into(),
        InboxItem::NoDriver { .. } => "word that no driver fits".into(),
        InboxItem::BudgetReached { lead } => {
            format!("word that {} reached its budget", n.agent(*lead))
        }
        InboxItem::Nudge { task, .. } => format!("a nudge on {}", n.task(*task)),
    }
}

fn wait_on(n: &Names, on: &WaitOn) -> String {
    match on {
        WaitOn::Job(job) => n.job(*job),
        WaitOn::Report(task) => format!("the report on {}", n.task(*task)),
        WaitOn::Reply { from, .. } => format!("a reply from {}", n.party(*from)),
        WaitOn::Decision(action) => format!("the builder's decision on {}", n.action(*action)),
    }
}

fn task_event(n: &Names, task: TaskId, event: &TaskEvent) -> String {
    let title = n.task(task);
    match event {
        TaskEvent::Delegated(a) => format!("Gave {title} to {}", n.agent(a.assignee)),
        TaskEvent::ProgressNoted(p) => match p.done {
            Some(done) => format!(
                "Progress on {title}, {}%: {}",
                done.value(),
                snippet(&p.note)
            ),
            None => format!("Progress on {title}: {}", snippet(&p.note)),
        },
        TaskEvent::EffortChanged(e) => {
            format!("Changed the effort {title} needs to {}", words::effort(*e))
        }
        TaskEvent::Reported(r) => format!(
            "Reported on {title}, {}: {}",
            match r.outcome {
                Outcome::Completed => "done",
                Outcome::Failed(_) => "failed",
            },
            snippet(&r.summary)
        ),
        TaskEvent::Reviewed(Verdict::Accept) => format!("Accepted the work on {title}"),
        TaskEvent::Reviewed(Verdict::SendBack { note, .. }) => {
            format!("Sent {title} back: {}", snippet(note))
        }
        TaskEvent::Merged { .. } => format!("Merged the work on {title}"),
        TaskEvent::MergeFailed { .. } => format!("The merge of {title} hit a conflict"),
    }
}

fn job(n: &Names, does: &ProjectJob) -> String {
    match does {
        ProjectJob::Wake { agent, note, .. } => {
            format!("a wake-up for {}: {}", n.agent(*agent), snippet(note))
        }
        ProjectJob::Delegate { to, title, .. } => {
            format!("the task {} for {}", quote(title), n.agent(*to))
        }
        ProjectJob::Routine(ProjectRoutine::EndWarmPeriod { driver }) => {
            format!("the end of {}'s warm period", n.driver_agent(*driver))
        }
        ProjectJob::Routine(ProjectRoutine::WaitDeadline { agent, .. }) => {
            format!("the deadline of {}'s wait", n.agent(*agent))
        }
        ProjectJob::Routine(ProjectRoutine::DriverWaitDeadline { agent }) => {
            format!("the deadline of {}'s wait for a driver", n.agent(*agent))
        }
    }
}

fn trial_change(n: &Names, change: &TrialChange) -> String {
    match change {
        TrialChange::EditSkill { skill, .. } => format!("a change to the skill {}", skill.name.0),
        TrialChange::NewSkill { skill, .. } => format!("a new skill, {}", skill.name.0),
        TrialChange::CompactMemory { agent, .. } => {
            format!("compacting {}'s memory", n.agent(*agent))
        }
    }
}

fn trial_event(n: &Names, trial: TrialId, event: &TrialEvent) -> String {
    let about = n
        .project
        .trial(trial)
        .map_or_else(|| "a trial".into(), |t| trial_change(n, &t.trial.change));
    match event {
        TrialEvent::Started => format!("Started the trial of {about}"),
        TrialEvent::RunFinished(r) => format!(
            "A {} run of the trial of {about} passed {} of {} checks",
            match r.arm {
                x100_core::trial::Arm::Baseline => "baseline",
                x100_core::trial::Arm::Changed => "changed",
            },
            r.checks_passed,
            r.checks_total
        ),
        TrialEvent::Scored(TrialVerdict::Supports) => format!("The trial of {about} supports it"),
        TrialEvent::Scored(TrialVerdict::DoesNotSupport) => {
            format!("The trial of {about} does not support it")
        }
        TrialEvent::Committed { .. } => format!("Committed {about}, which the trial supported"),
        TrialEvent::Discarded => format!("Dropped {about}"),
        TrialEvent::Stopped => format!("Stopped the trial of {about}"),
    }
}

fn quote(s: &str) -> String {
    format!("\u{201c}{s}\u{201d}")
}

/// The first line of `s`, cut to a length a log line can hold.
fn snippet(s: &str) -> String {
    let first = s.lines().next().unwrap_or("").trim();
    match first.char_indices().nth(90) {
        Some((i, _)) => format!("{}\u{2026}", &first[..i]),
        None => first.to_owned(),
    }
}

fn duration(d: std::time::Duration) -> String {
    match d.as_secs() {
        s if s < 90 => format!("{s} seconds"),
        s if s < 5_400 => format!("{} minutes", s / 60),
        s if s < 172_800 => format!("{} hours", s / 3_600),
        s => format!("{} days", s / 86_400),
    }
}

#[cfg(test)]
mod tests;
