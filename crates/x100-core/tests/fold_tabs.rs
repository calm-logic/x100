//! What each tab and view shows after the Fernleaf scenario, as a builder would see it.

use x100_core::action::{ActionKind, ActionState, AskMeAbout, ClassifiedBy, Risk};
use x100_core::agent::{AgentState, Waiting};
use x100_core::driver::{
    DriverState, EndReason, Failure, HandoffCause, InFlight, PlannedReason, StartReason,
    TakeoverCause,
};
use x100_core::effort::{Effort, EffortRange};
use x100_core::event::ProjectEvent;
use x100_core::fold::{ChatItem, ProjectPart, ProjectState, Record};
use x100_core::job::JobState;
use x100_core::memory::NoteName;
use x100_core::money::{Money, Percent};
use x100_core::party::{Actor, Party};
use x100_core::process::ProcessName;
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::task::TaskState;
use x100_core::testing::{Fernleaf, MACHINE, fernleaf, gpt, opus, sonnet};
use x100_core::trial::TrialState;
use x100_core::wait::WaitOn;

fn scenario() -> (Fernleaf, ProjectState) {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).expect("the scenario folds");
    (f, state)
}

fn range(min: Effort, max: Effort) -> EffortRange {
    EffortRange::new(min, max).unwrap()
}

fn dollars(micros: u64) -> Money {
    Money::from_micros(micros)
}

#[test]
fn the_team_tab_shows_the_tree_with_each_agents_task_driver_and_range() {
    let (f, s) = scenario();
    assert_eq!(s.children(f.ceo), vec![f.tech, f.product, f.marketing]);
    assert_eq!(s.children(f.tech), vec![f.backend, f.frontend]);
    assert_eq!(s.children(f.backend), vec![f.migrations]);
    assert_eq!(s.children(f.product), vec![f.research]);
    assert_eq!(s.children(f.marketing), vec![]);

    let ceo = s.row(f.ceo).unwrap();
    assert_eq!(ceo.agent.role, "CEO");
    assert_eq!(ceo.agent.state, AgentState::Waiting(Waiting::OnWaits));
    assert_eq!(ceo.task.unwrap().task.title, "Launch Fernleaf");
    assert_eq!(ceo.progress, Some(Percent::new(60).unwrap()));
    let driver = ceo.driver.unwrap();
    assert_eq!(
        (
            driver.driver.harness,
            &driver.driver.model,
            driver.driver.effort
        ),
        (opus().harness, &opus().model, Effort::High)
    );
    assert_eq!(ceo.effort, EffortRange::FULL);

    let tech = s.row(f.tech).unwrap();
    assert_eq!(tech.agent.role, "Tech");
    assert_eq!(tech.parent, Some(f.ceo));
    assert_eq!(tech.task.unwrap().task.title, "Build the site");
    assert_eq!(tech.progress, Some(Percent::new(75).unwrap()));
    assert_eq!(tech.driver.unwrap().driver.model, gpt().model);
    assert_eq!(tech.effort, range(Effort::Medium, Effort::Max));

    let marketing = s.row(f.marketing).unwrap();
    assert_eq!(marketing.agent.state, AgentState::Idle);
    assert!(marketing.task.is_none());
    assert_eq!(marketing.effort, range(Effort::Low, Effort::Medium));

    // A worker without a range of its own has its parent's.
    let backend = s.row(f.backend).unwrap();
    assert_eq!(backend.agent.state, AgentState::Working);
    assert_eq!(backend.task.unwrap().task.title, "Build the API");
    assert_eq!(backend.progress, Some(Percent::new(80).unwrap()));
    assert_eq!(backend.driver.unwrap().driver.effort, Effort::High);
    assert_eq!(backend.effort, range(Effort::Medium, Effort::Max));

    // Research's last driver retired at the end of its warm period.
    assert!(s.row(f.research).unwrap().driver.is_none());
    assert_eq!(
        s.trail(f.migrations),
        vec![f.ceo, f.tech, f.backend, f.migrations]
    );
}

#[test]
fn an_agents_view_shows_its_drivers_and_why_each_ended() {
    let (f, s) = scenario();
    let backend = s.agent(f.backend).unwrap();
    assert_eq!(
        backend.drivers,
        vec![f.failed_driver, f.takeover_driver, f.backend_driver]
    );
    let failed = s.driver(f.failed_driver).unwrap();
    assert_eq!(failed.state, DriverState::Failed(Failure::StreamBroke));
    assert!(failed.ended.is_some());

    let takeover = s.driver(f.takeover_driver).unwrap();
    let StartReason::Handoff(h) = &takeover.driver.why else {
        panic!("a handoff");
    };
    assert_eq!(h.replaces, f.failed_driver);
    assert_eq!(
        h.cause,
        HandoffCause::Takeover(TakeoverCause::Failed(Failure::StreamBroke))
    );
    assert_eq!(
        h.in_flight,
        InFlight::Command {
            command: "cargo test".into()
        }
    );
    // Another provider carried on.
    assert_ne!(takeover.driver.harness, failed.driver.harness);
    assert_eq!(
        takeover.state,
        DriverState::Ended(EndReason::HandedOff(PlannedReason::EffortChanged))
    );

    let current = s.driver(f.backend_driver).unwrap();
    let StartReason::Handoff(h) = &current.driver.why else {
        panic!("a handoff");
    };
    assert_eq!(h.replaces, f.takeover_driver);
    assert_eq!(h.cause, HandoffCause::Planned(PlannedReason::EffortChanged));
    assert_eq!(s.acting_driver(f.backend), Some(f.backend_driver));
    assert_eq!(
        backend.memories.keys().collect::<Vec<_>>(),
        vec![&NoteName("auth".into())]
    );
}

#[test]
fn an_agents_view_shows_what_it_waits_for_its_inbox_jobs_and_processes() {
    let (f, s) = scenario();
    let ceo = s.agent(f.ceo).unwrap();
    let mut waits: Vec<_> = ceo.waits.values().map(|w| w.on).collect();
    waits.sort_by_key(|on| matches!(on, WaitOn::Reply { .. }));
    assert_eq!(
        waits,
        vec![
            WaitOn::Report(f.site),
            WaitOn::Reply {
                message: f.budget_question,
                from: Party::Builder
            }
        ]
    );
    // Every item delivered to anyone has been handled by a completed turn.
    assert!(s.agents().all(|a| a.inbox.is_empty() && a.items.is_empty()));

    // The one-off recheck fired; the daily smoke tests fired once and the builder cancelled them.
    let recheck = s.job(f.recheck_job).unwrap();
    assert_eq!((recheck.state, recheck.fired), (JobState::Done, 1));
    let smoke = s.job(f.smoke_job).unwrap();
    assert_eq!((smoke.state, smoke.fired), (JobState::Cancelled, 1));
    assert_eq!(smoke.cancelled_by, Some(Actor::Builder));
    assert_eq!(s.jobs_of(f.ceo).count(), 0);

    let frontend = s.agent(f.frontend).unwrap();
    let running: Vec<_> = frontend.processes.keys().collect();
    assert_eq!(running, vec![&ProcessName("dev".into())]);
    assert_eq!(
        frontend.processes[&ProcessName("dev".into())].command,
        "npm run dev -- --port 4321"
    );
    // The dev server outlives the driver that started it.
    assert_eq!(frontend.state, AgentState::Idle);
}

#[test]
fn an_agents_view_shows_its_skills_and_the_trials_about_them() {
    let (f, s) = scenario();
    let tech = s.agent(f.tech).unwrap();
    assert_eq!(tech.skills.len(), 1);
    assert_eq!(tech.skills[0].skill.name.0, "deploy");
    let about_tech: Vec<_> = s.trials_about(f.tech).map(|t| t.trial.id).collect();
    assert_eq!(about_tech, vec![f.committed_trial]);
    let about_frontend: Vec<_> = s.trials_about(f.frontend).map(|t| t.trial.id).collect();
    assert_eq!(about_frontend, vec![f.committed_trial, f.discarded_trial]);

    let committed = s.trial(f.committed_trial).unwrap();
    assert_eq!(committed.state, TrialState::Committed);
    assert_eq!(committed.runs.len(), 6);
    assert!(committed.commit.is_some());
    let discarded = s.trial(f.discarded_trial).unwrap();
    assert_eq!(discarded.state, TrialState::Discarded);
    assert_eq!(s.skills().len(), 1);
    assert_eq!(s.skills()[0].trial, f.committed_trial);
}

#[test]
fn the_drivers_list_shows_every_live_driver() {
    let (f, s) = scenario();
    let live: Vec<_> = s.live_drivers().map(|d| d.driver.agent).collect();
    let mut expected = vec![f.ceo, f.tech, f.product, f.frontend, f.backend, f.marketing];
    let mut got = live.clone();
    got.sort();
    expected.sort();
    assert_eq!(got, expected);
    for d in s.live_drivers() {
        assert_eq!(d.driver.machine, MACHINE);
        assert_eq!(d.state, DriverState::Warm);
        assert_eq!(d.context.unwrap().window, 200_000);
        assert!(d.spent.cost > Money::ZERO);
    }
    let marketing = s
        .live_drivers()
        .find(|d| d.driver.agent == f.marketing)
        .unwrap();
    assert_eq!(
        (&marketing.driver.model, marketing.driver.effort),
        (&sonnet().model, Effort::Low)
    );
    assert_eq!(marketing.spent.cost, dollars(5_000_000));
}

#[test]
fn the_tasks_tab_lists_every_task_with_its_status_and_a_live_dot() {
    let (f, s) = scenario();
    let status = |t| s.task(t).unwrap().state.name();
    assert_eq!(status(f.bug), "NewIdea");
    assert_eq!(status(f.brief), "InProgress");
    assert_eq!(status(f.site), "InProgress");
    assert_eq!(status(f.api), "InProgress");
    assert_eq!(status(f.copy), "InReview");
    assert_eq!(status(f.landing), "Promoting");
    for done in [f.prd, f.competitors, f.schema, f.smoke] {
        assert_eq!(status(done), "Done");
    }
    assert_eq!(s.tasks().count(), 10);

    let live: Vec<_> = s.tasks().filter(|t| s.task_is_live(t.task.id)).collect();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].task.title, "Build the API");

    let bug = s.task(f.bug).unwrap();
    assert_eq!(bug.task.raised_by, Party::Agent(f.frontend));
    assert_eq!(bug.task.part.as_deref(), Some("Website"));
    assert_eq!(bug.task.attachments, vec![f.screenshot]);

    // The API was sent back asking for more effort.
    let api = s.task(f.api).unwrap();
    let TaskState::InProgress(a) = &api.state else {
        panic!("in progress");
    };
    assert_eq!(a.effort, Effort::High);
    assert_eq!(
        api.sent_back.as_deref(),
        Some("Add rate limiting to the public routes.")
    );
    assert!(s.task(f.schema).unwrap().merged.is_some());
}

#[test]
fn the_files_tab_shows_what_the_project_holds_and_the_log_the_removal() {
    let (f, s) = scenario();
    let files: Vec<_> = s.files().map(|r| r.file.id).collect();
    assert_eq!(files, vec![f.sketch, f.screenshot]);
    let sketch = s.file(f.sketch).unwrap();
    assert_eq!(sketch.file.name, "sketch.png");
    assert_eq!(sketch.file.media_type, "image/png");
    assert_eq!(sketch.file.size, 48_213);
    assert_eq!(sketch.file.added_by, Party::Builder);
    assert_eq!(
        s.file(f.screenshot).unwrap().file.added_by,
        Party::Agent(f.frontend)
    );
    assert!(s.file(f.competitors_png).is_none());
    assert!(f.log.iter().any(|e| e.by == Actor::Builder
        && e.event
            == ProjectEvent::FileRemoved {
                file: f.competitors_png
            }));
}

#[test]
fn the_budget_tab_shows_spend_against_each_budget() {
    let (f, s) = scenario();
    let b = s.budget().unwrap();
    assert_eq!(b.project.budget, Money::from_dollars(50));
    assert_eq!(b.project.spent, dollars(7_658_300));
    assert!(!b.project.stopped);

    // Lab runs: twelve runs at three cents, against a tenth of the total.
    assert_eq!(b.lab.spent, dollars(360_000));
    assert_eq!(b.lab.budget, Money::from_dollars(5));

    let tech = b.leads[&f.tech];
    assert_eq!(tech.budget, Money::from_dollars(18));
    // Tech, Backend, Frontend and Migrations.
    assert_eq!(tech.spent, dollars(1_235_000));
    assert!(!tech.stopped);
    let product = b.leads[&f.product];
    assert_eq!(product.spent, dollars(350_000));
    // Marketing's turns and the classifier call for its email.
    let marketing = b.leads[&f.marketing];
    assert_eq!(marketing.spent, dollars(5_003_300));
    assert!(marketing.stopped);

    assert_eq!(b.agents[&f.ceo].cost, dollars(710_000));
    assert_eq!(b.agents[&f.marketing].cost, dollars(5_000_000));
    assert_eq!(b.agents[&f.backend].cost, dollars(440_000));
    assert!(b.usage.input > 0 && b.usage.cache_read > b.usage.input);
}

#[test]
fn the_log_has_a_line_for_every_event_with_who_and_when() {
    let (f, s) = scenario();
    assert_eq!(s.seq().map(|s| s.0 as usize), Some(f.log.len()));
    assert!(f.log.len() >= 300, "a few hundred events");
    assert_eq!(f.log[0].by, Actor::Builder);
    assert!(f.log.windows(2).all(|w| w[0].at < w[1].at));
    let fired = |job| {
        f.log.iter().any(|e| {
            matches!(e.event, ProjectEvent::Job { job: j, event: x100_core::job::JobEvent::Fired { .. } } if j == job)
        })
    };
    assert!(fired(f.smoke_job) && fired(f.recheck_job));
    let handoffs = f
        .log
        .iter()
        .filter(|e| {
            matches!(&e.event, ProjectEvent::DriverStarted(d) if matches!(d.why, StartReason::Handoff(_)))
        })
        .count();
    assert_eq!(handoffs, 2);
}

#[test]
fn the_chat_holds_the_builder_and_ceo_messages_and_the_open_question() {
    let (f, s) = scenario();
    let messages: Vec<_> = s
        .chat()
        .iter()
        .map(|c| match &c.item {
            ChatItem::Message(m) => s.message(*m).unwrap().message.body.clone(),
            ChatItem::Report { report, .. } => report.summary.clone(),
        })
        .collect();
    assert_eq!(messages.len(), 5);
    assert!(messages[0].starts_with("A landing page for Fernleaf"));
    assert_eq!(messages[3], "Forest greens.");

    let brief = s.message(match s.chat()[0].item {
        ChatItem::Message(m) => m,
        _ => panic!("a message"),
    });
    assert_eq!(brief.unwrap().message.attachments, vec![f.sketch]);
    assert_eq!(s.message(f.question).unwrap().reply, Some(f.answer));
    let open: Vec<_> = s.open_questions().map(|m| m.message.id).collect();
    assert_eq!(open, vec![f.budget_question]);
    // The question between leads is not in the chat, and was answered.
    assert!(s.message(f.lead_question).unwrap().reply.is_some());
}

#[test]
fn held_actions_show_what_they_are_why_and_how_they_were_decided() {
    let (f, s) = scenario();
    assert_eq!(s.held_actions().count(), 0);
    let deploy = s.action(f.deploy).unwrap();
    assert_eq!(deploy.state, ActionState::CarriedOut);
    let c = deploy.classification.as_ref().unwrap();
    assert_eq!(
        (c.kind, c.risk),
        (ActionKind::DeployToProduction, Risk::High)
    );
    assert_eq!(c.why, "Deploys to production.");
    assert!(deploy.decided.is_some());

    let email = s.action(f.email).unwrap();
    assert_eq!(email.state, ActionState::Denied);
    let c = email.classification.as_ref().unwrap();
    assert_eq!(c.kind, ActionKind::SendOutside);
    assert!(matches!(c.by, ClassifiedBy::Classifier(_)));
}

#[test]
fn specs_settings_preview_and_the_sidebar() {
    let (f, s) = scenario();
    let brd = s.spec(SpecKind::Brd).unwrap();
    assert_eq!(
        (brd.spec.author, brd.spec.revision, brd.spec.status),
        (f.ceo, 2, SpecStatus::Approved)
    );
    let prd = s.spec(SpecKind::Prd).unwrap();
    assert_eq!(
        (prd.spec.author, prd.spec.revision, prd.spec.status),
        (f.product, 2, SpecStatus::Approved)
    );
    let add = s.spec(SpecKind::Add).unwrap();
    assert_eq!(
        (add.spec.author, add.spec.revision, add.spec.status),
        (f.tech, 1, SpecStatus::Draft)
    );

    let project = s.project().unwrap();
    assert_eq!(project.name, "Fernleaf");
    assert_eq!(project.settings.ask_me_about, AskMeAbout::MediumAndUp);
    assert_eq!(project.settings.allowed.len(), 3);
    assert_eq!(project.preview.as_deref(), Some("http://localhost:4321"));

    assert!(s.is_live());
    assert_eq!(s.agent_count(), 8);
}

#[test]
fn each_event_says_which_parts_of_the_interface_it_changed() {
    let f = fernleaf();
    let mut s = ProjectState::default();
    let mut seen_builder_message = false;
    for e in &f.log {
        let ch = s.apply(e).unwrap();
        assert!(ch.parts.contains(&ProjectPart::Log));
        match &e.event {
            ProjectEvent::Charged(_) => assert!(ch.parts.contains(&ProjectPart::Budget)),
            ProjectEvent::MessageSent(m) if e.by == Actor::Builder => {
                seen_builder_message = true;
                assert!(ch.parts.contains(&ProjectPart::Chat));
                assert!(ch.records.contains(&Record::Message(m.id)));
            }
            ProjectEvent::FileRemoved { file } => {
                assert!(ch.parts.contains(&ProjectPart::Files));
                assert!(ch.records.contains(&Record::File(*file)));
            }
            // A setting changes Settings and nothing else.
            ProjectEvent::SettingChanged(_) => {
                assert_eq!(
                    ch.parts.iter().copied().collect::<Vec<_>>(),
                    vec![ProjectPart::Log, ProjectPart::Settings]
                );
            }
            ProjectEvent::Driver { driver, .. } => {
                assert!(ch.records.contains(&Record::Driver(*driver)));
                assert!(!ch.parts.contains(&ProjectPart::Chat));
            }
            _ => {}
        }
    }
    assert!(seen_builder_message);
}
