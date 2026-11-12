//! The plan's M6 check, run for real: fake-harness processes playing a script modelled on the
//! Fernleaf demo, a real store and git, the orchestrator in this process on the fake clock.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use common::*;
use x100_core::agent::{AgentEvent, AgentState, Position};
use x100_core::driver::{DriverEvent, DriverState, EndReason, RetireReason};
use x100_core::event::{Envelope, ProjectEvent};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, InboxItemId, JobId, TaskId, WaitId};
use x100_core::inbox::InboxItem;
use x100_core::job::{JobState, ProjectJob, ProjectRoutine};
use x100_core::message::Route;
use x100_core::party::Party;
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::task::{Opening, Outcome, TaskState};
use x100_core::wait::{WaitEnd, WaitOn};

const SCRIPT: &str = include_str!("scripts/fernleaf.toml");
const DAY: Duration = Duration::from_secs(24 * 3600);

/// The whole run, from the brief to the second morning's smoke tests.
async fn run() -> World {
    // The router puts every agent on the cheaper model, whose warm period an hour outlasts.
    let w = World::new(Setup::new(SCRIPT)).await;
    w.brief("Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.")
        .await;
    w.settle().await;

    // Marketing has no work yet, so it has no driver.
    let s = w.state();
    let marketing = agent(&s, "Marketing");
    assert_eq!(s.agent(marketing).unwrap().state, AgentState::Planned);
    assert!(s.agent(marketing).unwrap().drivers.is_empty());

    // The CEO kept working while its question was open: it accepted the site meanwhile.
    let question = s
        .open_questions()
        .next()
        .expect("the CEO asked the builder")
        .message
        .id;
    let site = task(&s, "Build the site");
    assert!(matches!(s.task(site).unwrap().state, TaskState::Done(..)));
    w.orch
        .send_chat(w.project, "Forest greens.".into(), vec![], Some(question))
        .await
        .expect("the answer is sent");
    w.settle().await;

    // An hour on: Research's warm period is long over, and its job wakes it.
    w.clock_to(START + HOUR).await;
    w.settle().await;
    // The first morning's smoke tests, and the next day's.
    w.clock_to(START + 2 * HOUR).await;
    w.settle().await;
    w.clock_to(START + 2 * HOUR + DAY).await;
    w.settle().await;
    let smoke = w
        .state()
        .jobs()
        .find(|j| matches!(j.job.does, ProjectJob::Delegate { .. }))
        .map(|j| j.job.id)
        .expect("the daily job");
    w.orch
        .cancel_job(w.project, smoke)
        .await
        .expect("the builder cancels it");
    w.settle().await;
    w
}

#[tokio::test(flavor = "multi_thread")]
async fn fernleaf_runs_as_an_organisation() {
    let w = run().await;
    let s = w.state();
    let log = w.log();
    print_summary(&s, &log);

    let ceo = s.project().unwrap().ceo;
    let role = |r: &str| agent(&s, r);
    let (tech, product, marketing) = (role("Tech"), role("Product"), role("Marketing"));

    // A BRD written by the CEO, revised once the builder answered.
    let brd = s.spec(SpecKind::Brd).expect("a BRD");
    assert_eq!((brd.spec.author, brd.spec.revision), (ceo, 2));
    assert_eq!(brd.spec.status, SpecStatus::Approved);
    assert!(s.spec(SpecKind::Prd).is_some() && s.spec(SpecKind::Add).is_some());

    // Three leads, workers under two of them, and a worker with a worker of its own.
    let team = &s.project().unwrap().team;
    let leads: BTreeSet<_> = team
        .members()
        .filter(|(_, p)| matches!(p, Position::Lead { .. }))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(leads, BTreeSet::from([tech, product, marketing]));
    let under = |a: AgentId| -> BTreeSet<String> {
        s.children(a)
            .into_iter()
            .map(|c| s.agent(c).unwrap().role.clone())
            .collect()
    };
    assert_eq!(
        under(tech),
        ["Backend", "Frontend"].map(String::from).into()
    );
    assert_eq!(under(product), ["Research"].map(String::from).into());
    assert_eq!(
        under(role("Backend")),
        ["Migrations"].map(String::from).into()
    );
    assert!(under(marketing).is_empty());

    // No lead is started before work reaches it.
    for lead in [tech, product, marketing] {
        let first_item = first_seq(
            &log,
            |e| matches!(e, ProjectEvent::Agent { agent, event: AgentEvent::Delivered { .. } } if *agent == lead),
        );
        let first_driver = first_seq(
            &log,
            |e| matches!(e, ProjectEvent::DriverStarted(d) if d.agent == lead),
        );
        assert!(
            first_item < first_driver,
            "a lead started before it had work"
        );
    }
    let answered = first_seq(
        &log,
        |e| matches!(e, ProjectEvent::MessageSent(m) if m.route == Route::BuilderToCeo && m.in_reply_to.is_some()),
    );
    assert!(
        first_seq(
            &log,
            |e| matches!(e, ProjectEvent::DriverStarted(d) if d.agent == marketing)
        ) > answered,
        "Marketing's first work came with the builder's answer"
    );

    // A lead asks another lead directly, and the answer comes back as a reply.
    let between = |from: AgentId, to: AgentId| {
        s.messages()
            .find(|m| m.message.route == Route::Between { from, to })
    };
    let asked = between(product, tech).expect("Product asks Tech");
    let told = between(tech, product).expect("Tech answers Product");
    assert_eq!(told.message.in_reply_to, Some(asked.message.id));
    // Delegating to another lead was refused, and the refusal is in the log.
    assert!(s.actions().any(|a| a.action.agent == product
        && a.error.as_deref().is_some_and(|e| e.contains("by message"))));

    // A question to the builder, answered.
    let question = s
        .messages()
        .find(|m| m.message.route == Route::CeoToBuilder && m.awaited)
        .expect("a question to the builder");
    let answer = s.message(question.reply.expect("answered")).unwrap();
    assert_eq!(answer.message.route, Route::BuilderToCeo);

    // Tasks move through their statuses: a raised idea delegated, each delegated task reported,
    // accepted, merged and done, and the brief ending in the CEO's report, merged into main.
    let statuses: Vec<_> = log
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::TaskOpened {
                opening: Opening::Raised,
                ..
            } => Some("NewIdea"),
            ProjectEvent::Task { event, .. } => Some(event.name()),
            _ => None,
        })
        .collect();
    for status in [
        "NewIdea",
        "Delegated",
        "Reported",
        "Reviewed(Accept)",
        "Merged",
    ] {
        assert!(statuses.contains(&status), "no task went {status}");
    }
    for t in s.tasks() {
        assert!(
            matches!(t.state, TaskState::Done(..)),
            "{} is {}",
            t.task.title,
            t.state.name()
        );
    }
    let brief = task(&s, "Launch Fernleaf");
    let TaskState::Done(asg, report) = &s.task(brief).unwrap().state else {
        unreachable!()
    };
    assert_eq!(
        (asg.delegator, &report.outcome),
        (Party::Builder, &Outcome::Completed)
    );
    assert!(s.chat().iter().any(|c| matches!(
        &c.item,
        x100_core::fold::ChatItem::Report { task, .. } if *task == brief
    )));

    // The work is real: what the workers wrote is on the project's main branch.
    for file in [
        "site/index.html",
        "site/mobile.css",
        "api/waitlist.ts",
        "migrations/0001.sql",
        "copy/launch.md",
    ] {
        assert!(w.project_dir().join(file).exists(), "{file} is not on main");
    }

    // A worker waits on a job it scheduled and is woken when it fires, on a new driver, since
    // its first retired during the wait. The new one started from its start pack, with its
    // progress note.
    let research = role("Research");
    let recheck = s
        .jobs()
        .find(|j| {
            matches!(&j.job.does, ProjectJob::Wake { scheduled_by, agent, .. }
            if *scheduled_by == research && *agent == research)
        })
        .expect("Research's own job");
    assert_eq!((recheck.state, recheck.fired), (JobState::Done, 1));
    let waited = wait_on(&log, research, WaitOn::Job(recheck.job.id));
    assert_eq!(wait_end(&log, waited), Some(WaitEnd::Satisfied));
    let drivers = &s.agent(research).unwrap().drivers;
    assert!(drivers.len() >= 2);
    let first = s.driver(drivers[0]).unwrap();
    assert_eq!(
        first.state,
        DriverState::Ended(EndReason::Retired(RetireReason::WarmTooLong))
    );
    let pack = w
        .orch
        .start_pack(w.project, drivers[1])
        .expect("a start pack");
    assert!(pack.contains("Three found; pricing to recheck."), "{pack}");
    assert!(pack.contains("retired (WarmTooLong)"), "{pack}");
    let woken = given_in(&log, drivers[1]);
    assert!(
        woken.iter().any(
            |i| matches!(item_of(&log, *i), InboxItem::Job { job, .. } if job == recheck.job.id)
        )
    );

    // A lead waits on its worker's report and resumes when it arrives.
    let landing = task(&s, "Build the landing page");
    let tech_waited = wait_on(&log, tech, WaitOn::Report(landing));
    assert_eq!(wait_end(&log, tech_waited), Some(WaitEnd::Satisfied));
    let report_item = log
        .iter()
        .find_map(|e| match &e.event {
            ProjectEvent::Agent {
                agent,
                event:
                    AgentEvent::Delivered {
                        item,
                        what: InboxItem::Report { task },
                    },
            } if *agent == tech && *task == landing => Some(*item),
            _ => None,
        })
        .expect("the report reached Tech");
    assert!(
        handled(&log, report_item),
        "Tech took the report up in a turn that completed"
    );

    // A repeating job delegates a fresh task to a lead at each firing.
    let smoke = s
        .jobs()
        .find(|j| matches!(j.job.does, ProjectJob::Delegate { .. }))
        .unwrap();
    assert_eq!((smoke.fired, smoke.state), (2, JobState::Cancelled));
    let smoke_tasks: Vec<_> = s
        .tasks()
        .filter(|t| t.task.title == "Run the smoke tests")
        .collect();
    assert_eq!(smoke_tasks.len(), 2);
    for t in smoke_tasks {
        let asg = t.state.assignment().unwrap();
        assert_eq!((asg.delegator, asg.assignee), (Party::Agent(ceo), tech));
    }

    assert_invariants(&s, &log);
    w.finish().await;
}

/// The check's invariants, over any finished run.
fn assert_invariants(s: &ProjectState, log: &[Envelope<ProjectEvent>]) {
    // The orchestrator closes waits and cancels their deadline jobs.
    assert!(
        s.agents().all(|a| a.waits.is_empty()),
        "a wait was left open"
    );
    let opened: BTreeMap<WaitId, JobId> = log
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::Agent {
                event: AgentEvent::WaitOpened(w),
                ..
            } => Some((w.id, w.deadline_job)),
            _ => None,
        })
        .collect();
    assert!(!opened.is_empty());
    for (wait, deadline) in &opened {
        let end = wait_end(log, *wait).expect("every wait ended");
        let job = s.job(*deadline).unwrap();
        let expected = if end == WaitEnd::Expired {
            JobState::Done
        } else {
            JobState::Cancelled
        };
        assert_eq!(
            job.state, expected,
            "the deadline of a wait that ended {end:?}"
        );
    }
    for j in s.jobs() {
        if let ProjectJob::Routine(ProjectRoutine::WaitDeadline { .. }) = j.job.does {
            assert!(
                !matches!(j.state, JobState::Scheduled { .. }),
                "a deadline outlived its wait"
            );
        }
    }

    // Nothing delivered to an agent is lost: every item was handled by a turn that completed.
    assert!(
        s.agents().all(|a| a.inbox.is_empty()),
        "an item was left unhandled"
    );
    for e in log {
        if let ProjectEvent::Agent {
            event: AgentEvent::Delivered { item, .. },
            ..
        } = &e.event
        {
            assert!(
                handled(log, *item),
                "{item} was never handled by a completed turn"
            );
        }
    }

    // Every delegated task ends in a report to its delegator.
    for t in s.tasks() {
        let Some(asg) = t.state.assignment() else {
            continue;
        };
        assert!(
            !matches!(t.state, TaskState::InProgress(_)),
            "{} never reported",
            t.task.title
        );
        if let Party::Agent(d) = asg.delegator {
            let reached = log.iter().any(|e| matches!(&e.event,
                ProjectEvent::Agent { agent, event: AgentEvent::Delivered { what: InboxItem::Report { task }, .. } }
                    if *agent == d && *task == t.task.id));
            assert!(
                reached,
                "the report on {} did not reach its delegator",
                t.task.title
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_run_is_the_same_every_time() {
    let mut first: Option<Vec<String>> = None;
    for run_no in 0..10 {
        let w = run().await;
        let kinds = kinds(&w.log());
        w.finish().await;
        match &first {
            None => first = Some(kinds),
            Some(f) => assert_eq!(f, &kinds, "run {run_no} differs from the first"),
        }
    }
}

fn task(s: &ProjectState, title: &str) -> TaskId {
    s.tasks()
        .find(|t| t.task.title == title)
        .map(|t| t.task.id)
        .unwrap_or_else(|| panic!("no task {title}"))
}

fn first_seq(log: &[Envelope<ProjectEvent>], f: impl Fn(&ProjectEvent) -> bool) -> u64 {
    log.iter()
        .find(|e| f(&e.event))
        .map(|e| e.seq.0)
        .unwrap_or(u64::MAX)
}

fn wait_on(log: &[Envelope<ProjectEvent>], agent: AgentId, on: WaitOn) -> WaitId {
    log.iter()
        .find_map(|e| match &e.event {
            ProjectEvent::Agent {
                agent: a,
                event: AgentEvent::WaitOpened(w),
            } if *a == agent && w.on == on => Some(w.id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no wait on {on:?}"))
}

fn wait_end(log: &[Envelope<ProjectEvent>], wait: WaitId) -> Option<WaitEnd> {
    log.iter().find_map(|e| match &e.event {
        ProjectEvent::Agent {
            event: AgentEvent::WaitEnded { wait: w, end },
            ..
        } if *w == wait => Some(*end),
        _ => None,
    })
}

fn given_in(log: &[Envelope<ProjectEvent>], driver: x100_core::id::DriverId) -> Vec<InboxItemId> {
    log.iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::Driver {
                driver: d,
                event: DriverEvent::TurnStarted { given },
            } if *d == driver => Some(given.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn item_of(log: &[Envelope<ProjectEvent>], item: InboxItemId) -> InboxItem {
    log.iter()
        .find_map(|e| match &e.event {
            ProjectEvent::Agent {
                event: AgentEvent::Delivered { item: i, what },
                ..
            } if *i == item => Some(what.clone()),
            _ => None,
        })
        .expect("a delivered item")
}

/// Whether `item` was given to a turn that then completed on the same driver.
fn handled(log: &[Envelope<ProjectEvent>], item: InboxItemId) -> bool {
    let mut holder = None;
    for e in log {
        match &e.event {
            ProjectEvent::Driver {
                driver,
                event: DriverEvent::TurnStarted { given },
            } if given.contains(&item) => {
                holder = Some(*driver);
            }
            ProjectEvent::Driver {
                driver,
                event: DriverEvent::TurnEnded,
            } if holder == Some(*driver) => return true,
            ProjectEvent::Driver {
                driver,
                event: DriverEvent::Failed(_) | DriverEvent::Stopped(_),
            } if holder == Some(*driver) => {
                holder = None;
            }
            _ => {}
        }
    }
    false
}

/// What the run produced, for the report: events by kind, and the team tree.
fn print_summary(s: &ProjectState, log: &[Envelope<ProjectEvent>]) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for k in kinds(log) {
        *counts.entry(k).or_default() += 1;
    }
    eprintln!("Fernleaf: {} events", log.len());
    for (k, n) in &counts {
        eprintln!("  {n:4} {k}");
    }
    fn tree(s: &ProjectState, a: AgentId, depth: usize) {
        let r = s.agent(a).unwrap();
        eprintln!(
            "{}{} ({}, {} driver(s), {})",
            "  ".repeat(depth + 1),
            r.role,
            r.state.name(),
            r.drivers.len(),
            if r.drivers.is_empty() {
                "never started".into()
            } else {
                format!(
                    "{} turn(s)",
                    r.drivers
                        .iter()
                        .filter_map(|d| s.driver(*d))
                        .map(|d| d.turns)
                        .sum::<u32>()
                )
            }
        );
        for c in s.children(a) {
            tree(s, c, depth + 1);
        }
    }
    eprintln!("Team:");
    tree(s, s.project().unwrap().ceo, 0);
}
