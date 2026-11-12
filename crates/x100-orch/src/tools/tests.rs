//! Who may call what, decided from the folded state alone. The state is core's Fernleaf scenario:
//! a CEO, three leads, workers under two of them and a worker under a worker.

use std::time::Duration;

use x100_core::effort::{Effort, EffortRange};
use x100_core::event::{Envelope, Seq};
use x100_core::file::ContentHash;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, JobId, TaskId};
use x100_core::job::{Interval, Schedule};
use x100_core::memory::NoteName;
use x100_core::money::{Money, Percent};
use x100_core::process::ProcessName;
use x100_core::settings::DaemonSettings;
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::task::{Progress, TaskState, Verdict};
use x100_core::team::LeadSpec;
use x100_core::testing::{Fernleaf, fernleaf};
use x100_core::time::{Clock, FakeClock};
use x100_core::tool::{
    AgentOutcome, Delegated, Delegatee, JobRequest, Proposal, Recipient, ToolCall,
};
use x100_core::wait::WaitOn;

use super::{Ctx, Plan, plan};

struct Scene {
    f: Fernleaf,
    state: ProjectState,
    clock: FakeClock,
    settings: DaemonSettings,
}

fn scene() -> Scene {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).expect("the scenario folds");
    let at = f.log.last().unwrap().at;
    Scene {
        f,
        state,
        clock: FakeClock::new(at),
        settings: DaemonSettings::default(),
    }
}

impl Scene {
    fn call(&self, agent: AgentId, call: &ToolCall) -> Result<Plan, String> {
        let ctx = Ctx {
            agent,
            now: self.clock.now(),
            settings: &self.settings,
            clock: &self.clock,
            file_stored: false,
        };
        plan(&self.state, &ctx, call)
    }

    fn refused(&self, agent: AgentId, call: &ToolCall) -> String {
        match self.call(agent, call) {
            Err(e) => e,
            Ok(p) => panic!("{call:?} was allowed: {:?}", p.events),
        }
    }

    /// Applies an allowed call's events to the state, as the store would.
    fn folds(&self, agent: AgentId, call: &ToolCall) -> ProjectState {
        let plan = self.call(agent, call).expect("allowed");
        let mut state = self.state.clone();
        let mut seq = state.seq().unwrap().0;
        for (by, event) in plan.events {
            seq += 1;
            let e = Envelope {
                seq: Seq(seq),
                at: self.clock.now(),
                by,
                event,
            };
            state.apply(&e).expect("the call's events fold");
        }
        state
    }
}

fn new_task(title: &str) -> Delegated {
    Delegated::New {
        title: title.into(),
        ask: "Do it.".into(),
        attachments: vec![],
    }
}

fn delegate(to: Delegatee, effort: Effort) -> ToolCall {
    ToolCall::Delegate {
        to,
        task: new_task("A task"),
        effort,
    }
}

fn progress() -> Progress {
    Progress {
        note: "Halfway.".into(),
        done: Some(Percent::new(50).unwrap()),
    }
}

#[test]
fn only_the_ceo_forms_the_team_and_only_the_first_time() {
    let s = scene();
    let call = ToolCall::FormTeam {
        leads: vec![LeadSpec {
            role: "Ops".into(),
            charter: "Ops.".into(),
            effort: EffortRange::FULL,
            budget: Money::from_dollars(1),
        }],
    };
    assert!(
        s.refused(s.f.tech, &call)
            .contains("only the CEO forms the team")
    );
    assert!(s.refused(s.f.backend, &call).contains("only the CEO"));
    assert!(s.refused(s.f.ceo, &call).contains("formed already"));
}

#[test]
fn only_the_ceo_sets_effort_and_budgets_and_budgets_stay_inside_the_total() {
    let s = scene();
    let effort = ToolCall::SetEffort {
        agent: s.f.backend,
        range: EffortRange::FULL,
    };
    assert!(
        s.refused(s.f.tech, &effort)
            .contains("only the CEO sets effort")
    );
    assert!(s.call(s.f.ceo, &effort).is_ok());
    let budget = |lead, dollars| ToolCall::SetBudget {
        lead,
        budget: Money::from_dollars(dollars),
    };
    assert!(
        s.refused(s.f.tech, &budget(s.f.tech, 1))
            .contains("only the CEO sets budgets")
    );
    assert!(
        s.refused(s.f.ceo, &budget(s.f.tech, 500))
            .contains("more than the project's total")
    );
    assert!(
        s.refused(s.f.ceo, &budget(s.f.backend, 1))
            .contains("not a lead")
    );
    s.folds(s.f.ceo, &budget(s.f.tech, 10));
}

#[test]
fn only_the_ceo_messages_the_builder() {
    let s = scene();
    let to_builder = ToolCall::Message {
        to: Recipient::Builder,
        body: "Hello".into(),
        attachments: vec![],
        in_reply_to: None,
    };
    assert!(
        s.refused(s.f.tech, &to_builder)
            .contains("only the CEO messages the builder")
    );
    assert!(
        s.refused(s.f.research, &to_builder)
            .contains("only the CEO")
    );
    s.folds(s.f.ceo, &to_builder);
    // Leads message each other directly.
    let lead_to_lead = ToolCall::Message {
        to: Recipient::Agent(s.f.product),
        body: "Which framework?".into(),
        attachments: vec![],
        in_reply_to: None,
    };
    let after = s.folds(s.f.tech, &lead_to_lead);
    assert!(!after.agent(s.f.product).unwrap().inbox.is_empty());
}

#[test]
fn an_agent_delegates_only_to_its_own_agents_or_a_new_worker() {
    let s = scene();
    let research = Delegatee::Child(s.f.research);
    assert!(
        s.refused(s.f.tech, &delegate(research, Effort::Low))
            .contains("not one of your agents")
    );
    let lead = s.refused(
        s.f.tech,
        &delegate(Delegatee::Child(s.f.product), Effort::Low),
    );
    assert!(lead.contains("by message"), "{lead}");
    assert!(
        s.refused(s.f.tech, &delegate(Delegatee::Child(s.f.tech), Effort::Low))
            .contains("itself")
    );
    // Down the tree, the task's effort is clamped into the assignee's range.
    let after = s.folds(
        s.f.backend,
        &delegate(Delegatee::Child(s.f.migrations), Effort::Max),
    );
    let opened = after
        .tasks()
        .find(|t| t.task.title == "A task")
        .expect("opened");
    let range = after
        .project()
        .unwrap()
        .team
        .effort(s.f.migrations)
        .unwrap();
    assert_eq!(opened.state.assignment().unwrap().effort, range.max());
    // A new worker is started under the delegator, with its range unless the CEO sets one.
    let worker = |effort| Delegatee::NewWorker {
        role: "QA".into(),
        charter: "Test it.".into(),
        effort,
    };
    let after = s.folds(s.f.frontend, &delegate(worker(None), Effort::Low));
    let qa = after.agents().find(|a| a.role == "QA").expect("started").id;
    assert_eq!(after.project().unwrap().team.parent(qa), Some(s.f.frontend));
    let ranged = s.refused(
        s.f.frontend,
        &delegate(worker(Some(EffortRange::FULL)), Effort::Low),
    );
    assert!(ranged.contains("only the CEO sets a worker's effort range"));
    s.folds(
        s.f.ceo,
        &delegate(worker(Some(EffortRange::FULL)), Effort::Low),
    );
}

#[test]
fn a_raised_task_is_delegated_only_while_nobody_holds_it() {
    let s = scene();
    let raised = |task| ToolCall::Delegate {
        to: Delegatee::Child(s.f.frontend),
        task: Delegated::Raised(task),
        effort: Effort::Low,
    };
    let after = s.folds(s.f.tech, &raised(s.f.bug));
    assert!(matches!(
        after.task(s.f.bug).unwrap().state,
        TaskState::InProgress(_)
    ));
    assert!(
        s.refused(s.f.tech, &raised(s.f.site))
            .contains("only a raised task")
    );
}

#[test]
fn an_agent_schedules_only_for_itself_and_its_own_agents() {
    let s = scene();
    let once = Schedule::Once {
        at: s.clock.now() + Duration::from_secs(60),
    };
    let wake = |agent| ToolCall::Schedule {
        when: once,
        does: JobRequest::Wake {
            agent,
            note: "Look again.".into(),
        },
    };
    assert!(
        s.refused(s.f.tech, &wake(s.f.research))
            .contains("not one of your agents")
    );
    s.folds(s.f.tech, &wake(s.f.tech));
    s.folds(s.f.tech, &wake(s.f.backend));
    let job_to = |to| ToolCall::Schedule {
        when: once,
        does: JobRequest::Delegate {
            to,
            title: "Smoke tests".into(),
            ask: "Run them.".into(),
            effort: Effort::Low,
        },
    };
    assert!(
        s.refused(s.f.ceo, &job_to(s.f.backend))
            .contains("not one of your agents")
    );
    assert!(
        s.refused(s.f.tech, &job_to(s.f.tech))
            .contains("not one of your agents")
    );
    s.folds(s.f.ceo, &job_to(s.f.tech));
    let often = ToolCall::Schedule {
        when: Schedule::Every {
            first: s.clock.now(),
            every: Interval::new(Duration::from_secs(60)).unwrap(),
        },
        does: JobRequest::Wake {
            agent: s.f.tech,
            note: "Again.".into(),
        },
    };
    assert!(s.refused(s.f.tech, &often).contains("too often"));
}

#[test]
fn an_agent_cancels_only_jobs_it_scheduled_and_only_while_they_can_fire() {
    let s = scene();
    let cancel = |job| ToolCall::CancelJob { job };
    assert!(
        s.refused(s.f.tech, &cancel(s.f.smoke_job))
            .contains("you did not schedule")
    );
    assert!(
        s.refused(s.f.ceo, &cancel(s.f.smoke_job))
            .contains("not scheduled")
    );
    assert!(
        s.refused(s.f.ceo, &cancel(JobId::from_u128(77)))
            .contains("no job")
    );
}

#[test]
fn only_the_delegator_reviews_and_only_a_reported_task() {
    let s = scene();
    let in_review = s
        .state
        .tasks()
        .find(|t| matches!(t.state, TaskState::InReview(..)))
        .expect("a task in review");
    let review = |task| ToolCall::Review {
        task,
        verdict: Verdict::Accept,
    };
    assert!(
        s.refused(s.f.tech, &review(in_review.task.id))
            .contains("you did not delegate")
    );
    assert!(
        s.refused(s.f.tech, &review(s.f.api))
            .contains("only a reported task")
    );
    let send_back = ToolCall::Review {
        task: in_review.task.id,
        verdict: Verdict::SendBack {
            note: "Warmer.".into(),
            effort: Some(Effort::Max),
        },
    };
    let delegator = match in_review.state.assignment().unwrap().delegator {
        x100_core::party::Party::Agent(a) => a,
        x100_core::party::Party::Builder => panic!("an agent's task"),
    };
    let after = s.folds(delegator, &send_back);
    assert!(matches!(
        after.task(in_review.task.id).unwrap().state,
        TaskState::InProgress(_)
    ));
    // Accepting names the merge of the assignee's branch into the delegator's.
    let accept = s.call(delegator, &review(in_review.task.id)).unwrap();
    assert!(accept.merge.is_some_and(|m| m.by == delegator));
}

#[test]
fn only_the_assignee_reports_updates_or_waits_on_its_task() {
    let s = scene();
    let report = |task| ToolCall::Report {
        task,
        outcome: AgentOutcome::Completed,
        summary: "Done.".into(),
    };
    assert!(
        s.refused(s.f.product, &report(s.f.api))
            .contains("not yours")
    );
    assert!(
        s.refused(s.f.backend, &report(s.f.smoke))
            .contains("not in progress")
    );
    let after = s.folds(s.f.backend, &report(s.f.api));
    assert!(matches!(
        after.task(s.f.api).unwrap().state,
        TaskState::InReview(..)
    ));
    let update = ToolCall::UpdateTask {
        task: s.f.api,
        progress: progress(),
        effort: None,
    };
    assert!(s.refused(s.f.tech, &update).contains("not yours"));
    let wait = |agent_task: TaskId, on| ToolCall::Wait {
        on,
        deadline: None,
        task: agent_task,
        progress: progress(),
    };
    let on_job = WaitOn::Job(s.f.smoke_job);
    assert!(
        s.refused(s.f.tech, &wait(s.f.api, on_job))
            .contains("not yours")
    );
    let unknown = WaitOn::Report(TaskId::from_u128(42));
    assert!(
        s.refused(s.f.backend, &wait(s.f.api, unknown))
            .contains("no task")
    );
    let not_mine = WaitOn::Report(s.f.copy);
    assert!(
        s.refused(s.f.backend, &wait(s.f.api, not_mine))
            .contains("did not delegate")
    );
}

#[test]
fn specs_files_and_previews_follow_their_rules() {
    let s = scene();
    let spec = |spec| ToolCall::WriteSpec {
        spec,
        status: SpecStatus::Draft,
        body: "# Draft".into(),
    };
    assert!(
        s.refused(s.f.tech, &spec(SpecKind::Brd))
            .contains("only the CEO writes the BRD")
    );
    assert!(
        s.refused(s.f.backend, &spec(SpecKind::Prd))
            .contains("the CEO or a lead")
    );
    let after = s.folds(s.f.tech, &spec(SpecKind::Add));
    let before = s.state.spec(SpecKind::Add).map_or(0, |r| r.spec.revision);
    assert_eq!(after.spec(SpecKind::Add).unwrap().spec.revision, before + 1);
    let file = ToolCall::AddFile {
        name: "shot.png".into(),
        media_type: "image/png".into(),
        size: 3,
        hash: ContentHash("sha256:none".into()),
    };
    assert!(s.refused(s.f.research, &file).contains("file store"));
    let empty = ToolCall::SetPreview { url: " ".into() };
    assert!(s.refused(s.f.frontend, &empty).contains("address"));
    let raise = |attachments| ToolCall::RaiseTask {
        title: "Dark mode".into(),
        ask: "Add it.".into(),
        part: None,
        attachments,
    };
    let ghost = x100_core::id::FileId::from_u128(404);
    assert!(
        s.refused(s.f.research, &raise(vec![ghost]))
            .contains("no file")
    );
    let after = s.folds(s.f.research, &raise(vec![s.f.sketch]));
    assert!(
        after
            .tasks()
            .any(|t| t.task.title == "Dark mode" && t.state == TaskState::NewIdea)
    );
    let preview = ToolCall::SetPreview {
        url: "http://localhost:4321".into(),
    };
    let after = s.folds(s.f.frontend, &preview);
    assert_eq!(
        after.project().unwrap().preview.as_deref(),
        Some("http://localhost:4321")
    );
}

#[test]
fn tools_of_later_milestones_say_which_one() {
    let s = scene();
    let cases = [
        (
            ToolCall::StartProcess {
                name: ProcessName("dev".into()),
                command: "npm run dev".into(),
            },
            "M10",
        ),
        (
            ToolCall::Remember {
                note: NoteName("auth".into()),
                body: "KV.".into(),
            },
            "M13",
        ),
        (ToolCall::ReadMemories { agent: s.f.tech }, "M13"),
        (
            ToolCall::ReadLog {
                from: Seq(1),
                to: None,
            },
            "M14",
        ),
        (
            ToolCall::ProposeChange(Proposal::Team {
                changes: vec![],
                telemetry: String::new(),
                team_toml: String::new(),
            }),
            "M14",
        ),
    ];
    for (call, milestone) in cases {
        let why = s.refused(s.f.ceo, &call);
        assert!(
            why.contains("not available yet") && why.contains(milestone),
            "{why}"
        );
    }
}
