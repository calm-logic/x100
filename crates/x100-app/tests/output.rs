//! An agent's view in the window, against a real daemon running the Fernleaf script on the fake
//! clock: its output arriving as the script runs, one continuous output across a change of
//! driver with the change marked, its waits and jobs as the client holds them, and a job
//! cancelled from the view.

mod common;

use common::*;
use gpui::{Entity, VisualTestContext, WindowHandle};
use x100_app::shell::Shell;
use x100_app::team::{AgentOutput, FactsModel, SegmentShown, TeamView};
use x100_client::Client;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, ProjectId};
use x100_core::job::JobState;
use x100_core::party::Actor;
use x100_proto::SetClock;

const BRIEF: &str = "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.";
const HOUR: std::time::Duration = std::time::Duration::from_secs(3600);

fn team(
    window: WindowHandle<Shell>,
    vcx: &VisualTestContext,
    project: ProjectId,
) -> Entity<TeamView> {
    vcx.read(|cx| {
        let main = window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .main
            .clone();
        main.read(cx).team().clone()
    })
}

fn state(client: &Client, project: ProjectId) -> ProjectState {
    client
        .project(project)
        .expect("the client holds the project")
}

fn role(p: &ProjectState, name: &str) -> AgentId {
    p.agents()
        .find(|a| a.role == name)
        .unwrap_or_else(|| panic!("no agent {name}"))
        .id
}

fn output(vcx: &VisualTestContext, team: &Entity<TeamView>) -> Entity<AgentOutput> {
    vcx.read(|cx| team.read(cx).output())
        .expect("the agent's output is shown")
}

fn shown(vcx: &VisualTestContext, output: &Entity<AgentOutput>) -> Vec<SegmentShown> {
    vcx.read(|cx| output.read(cx).shown(cx))
}

fn facts(vcx: &VisualTestContext, output: &Entity<AgentOutput>) -> Option<FactsModel> {
    vcx.read(|cx| output.read(cx).facts().read(cx).shown().cloned())
}

/// Clicks a row of the Team tab as soon as it is listed, to catch its agent at work.
fn open(vcx: &mut VisualTestContext, row: &'static str) {
    let until = std::time::Instant::now() + WAIT;
    while bounds(vcx, row).is_none() {
        vcx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(1));
        assert!(std::time::Instant::now() < until, "{row} was never listed");
    }
    let at = bounds(vcx, row).unwrap().center();
    vcx.simulate_click(at, gpui::Modifiers::none());
    vcx.run_until_parked();
}

/// The window, with the brief sent from another client and the main panel open.
fn started(run: &mut Run) -> (Client, ProjectId, WindowHandle<Shell>, VisualTestContext) {
    let client = run.client();
    let project = run.fernleaf_project(&client);
    let (window, mut vcx) = run.open(client.clone());
    run.rt
        .block_on(client.send_brief(project, BRIEF, vec![]))
        .expect("the brief is sent");
    pump_until(&mut vcx, "the main panel opens", |vcx| {
        bounds(vcx, "tab-Team").is_some()
    });
    (client, project, window, vcx)
}

#[test]
fn an_agent_three_levels_down_shows_its_output_as_it_works() {
    let mut run = Run::start();
    let (client, project, window, mut vcx) = started(&mut run);
    open(&mut vcx, "agent-Tech");
    open(&mut vcx, "agent-Backend");
    open(&mut vcx, "agent-Migrations");
    let team = team(window, &vcx, project);
    let output = output(&vcx, &team);

    // Each different log seen while the script runs, until Migrations' turn has ended.
    let mut seen: Vec<String> = Vec::new();
    pump_until(&mut vcx, "Migrations' turn is shown", |vcx| {
        let text = shown(vcx, &output)
            .first()
            .map(|s| s.text.clone())
            .unwrap_or_default();
        let done = text.contains("── turn ended ──");
        if seen.last() != Some(&text) {
            seen.push(text);
        }
        done
    });
    run.settle(&client, project);
    eprintln!(
        "Migrations' log took {} different states while it was open",
        seen.len()
    );

    let p = state(&client, project);
    let migrations = role(&p, "Migrations");
    assert_eq!(vcx.read(|cx| output.read(cx).agent()), migrations);
    let segments = shown(&vcx, &output);
    assert_eq!(segments.len(), 1, "{segments:#?}");
    let log = &segments[0].text;
    let order = [
        "━━ Driver 1 started: fake-fernleaf · fake-brief · ",
        "for the task “Write the schema migrations”.",
        "started from a start pack of",
        "▸ turn started",
        "▸ write migrations/0001.sql",
        "▸ report ",
        "usage: 700 in · 200 out",
        "── turn ended ──",
    ];
    let mut at = 0;
    for part in order {
        let found = log[at..]
            .find(part)
            .unwrap_or_else(|| panic!("{part:?} is not where it belongs in\n{log}"));
        at += found + part.len();
    }
    // The facts are the client's, once the window has taken in what it holds.
    pump_until(&mut vcx, "the facts follow the state", |vcx| {
        let p = state(&client, project);
        facts(vcx, &output) == FactsModel::read(&p, &client.state().daemon, migrations)
    });
}

#[test]
fn research_carries_on_across_two_drivers_with_the_change_marked() {
    let mut run = Run::start();
    let (client, project, window, mut vcx) = started(&mut run);
    run.settle(&client, project);
    open(&mut vcx, "agent-Product");
    open(&mut vcx, "agent-Research");
    let team = team(window, &vcx, project);
    let output = output(&vcx, &team);
    pump_until(&mut vcx, "Research's first turn is shown", |vcx| {
        shown(vcx, &output)
            .first()
            .is_some_and(|s| s.text.contains("── turn ended ──"))
    });

    // What it waits for and the job it scheduled, as the client holds them.
    let p = state(&client, project);
    let research = role(&p, "Research");
    let daemon = client.state().daemon;
    let model = facts(&vcx, &output).expect("facts");
    assert_eq!(
        Some(&model),
        FactsModel::read(&p, &daemon, research).as_ref()
    );
    let wait = p.agent(research).unwrap().waits.values().next().unwrap();
    assert_eq!(
        model.waits,
        [format!(
            "its job: Pricing pages update at noon; look again., until {}",
            x100_app::words::moment(wait.deadline)
        )]
    );
    let job = p.jobs_of(research).next().expect("its job");
    assert_eq!(model.jobs.len(), 1);
    assert_eq!(model.jobs[0].job, job.job.id);
    assert!(
        model.jobs[0]
            .text
            .starts_with("Wake itself: Pricing pages update at noon; look again. · at "),
        "{:?}",
        model.jobs[0]
    );

    // An hour on, the job wakes Research on a new driver. Its output carries on in the same log,
    // live, with the change marked.
    let before = shown(&vcx, &output)[0].text.clone();
    run.rt
        .block_on(client.request(SetClock { at: START + HOUR }))
        .expect("the clock moves");
    run.settle(&client, project);
    pump_until(&mut vcx, "the second driver's report is shown", |vcx| {
        shown(vcx, &output)
            .first()
            .is_some_and(|s| s.text.contains("Two of three price per seat."))
    });
    let segments = shown(&vcx, &output);
    assert_eq!(segments.len(), 1, "one continuous log: {segments:#?}");
    let log = &segments[0].text;
    assert!(log.starts_with(before.trim_end()), "{log}");
    let marker = log
        .lines()
        .find(|l| l.contains("Driver 2 took over"))
        .unwrap_or_else(|| panic!("no change marked in\n{log}"));
    assert_eq!(
        marker,
        "━━ Driver 2 took over: fake-fernleaf · fake-brief · Low, woken by its inbox. \
         Driver 1 ended: retired, warm too long."
    );
    assert!(log.find(marker).unwrap() < log.find("Two of three price per seat.").unwrap());
    pump_until(&mut vcx, "the facts follow the state", |vcx| {
        let p = state(&client, project);
        facts(vcx, &output) == FactsModel::read(&p, &client.state().daemon, research)
    });
    let drivers = facts(&vcx, &output).unwrap().drivers;
    assert_eq!(drivers.len(), 2, "{drivers:?}");
    assert!(
        drivers[1].ends_with("· retired, warm too long"),
        "{drivers:?}"
    );
}

#[test]
fn a_job_cancelled_from_the_agents_view_reaches_the_daemon() {
    let mut run = Run::start();
    let (client, project, _window, mut vcx) = started(&mut run);
    run.settle(&client, project);
    open(&mut vcx, "agent-Product");
    open(&mut vcx, "agent-Research");
    pump_until(&mut vcx, "the job's Cancel is drawn", |vcx| {
        bounds(vcx, "cancel-job-0").is_some()
    });
    let p = state(&client, project);
    let research = role(&p, "Research");
    let job = p.jobs_of(research).next().expect("its job").job.id;
    click(&mut vcx, "cancel-job-0");
    run.rt
        .block_on(client.wait_for(WAIT, |s| {
            let j = s.projects.get(&project)?.job(job)?;
            (j.state == JobState::Cancelled).then_some(j.cancelled_by)
        }))
        .map(|by| assert_eq!(by, Some(Actor::Builder)))
        .expect("the daemon cancelled the job");
    pump_until(&mut vcx, "the job leaves the view", |vcx| {
        bounds(vcx, "cancel-job-0").is_none()
    });
}
