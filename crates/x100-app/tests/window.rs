//! The app's window against a real daemon running the Fernleaf script, without a display: the
//! brief typed and sent, the Team tab following the state, opening agents, the drivers list, the
//! tabs' dots, rows redrawn only when their own agent changes, and a new client seeing the same.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use gpui::{Entity, VisualTestContext, WindowHandle};
use x100_app::shell::Shell;
use x100_app::tabs::{MainPanel, Tab};
use x100_app::team::{AgentModel, DriverModel, Listing, TeamView};
use x100_client::Client;
use x100_core::agent::Position;
use x100_core::effort::{Effort, EffortRange};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, ProjectId};
use x100_core::message::Route;

/// A one-pixel PNG, to attach as an image.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0xf8, 0xcf, 0xc0,
    0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn main_panel(
    window: WindowHandle<Shell>,
    vcx: &VisualTestContext,
    project: ProjectId,
) -> Entity<MainPanel> {
    vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .main
            .clone()
    })
}

fn team(
    window: WindowHandle<Shell>,
    vcx: &VisualTestContext,
    project: ProjectId,
) -> Entity<TeamView> {
    let main = main_panel(window, vcx, project);
    vcx.read(|cx| main.read(cx).team().clone())
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

/// What the Team tab's rows show now, by name, and how often each has been drawn.
fn rows(vcx: &VisualTestContext, team: &Entity<TeamView>) -> Vec<(AgentId, AgentModel, usize)> {
    vcx.read(|cx| {
        team.read(cx)
            .rows()
            .iter()
            .map(|r| {
                let r = r.read(cx);
                (r.agent(), r.shown().cloned().expect("drawn"), r.renders())
            })
            .collect()
    })
}

fn names(rows: &[(AgentId, AgentModel, usize)]) -> BTreeSet<String> {
    rows.iter().map(|(_, m, _)| m.name.clone()).collect()
}

/// Pumps until every row the Team tab lists shows what the client's state holds for it.
fn rows_follow_state(
    vcx: &mut VisualTestContext,
    team: &Entity<TeamView>,
    client: &Client,
    project: ProjectId,
) {
    pump_until(vcx, "the rows show the state", |vcx| {
        let p = state(client, project);
        let daemon = client.state().daemon;
        rows(vcx, team)
            .iter()
            .all(|(agent, shown, _)| AgentModel::read(&p, &daemon, *agent).as_ref() == Some(shown))
    });
}

#[test]
fn the_brief_opens_the_team_tab_which_follows_the_organisation() {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
    let (window, mut vcx) = run.open(client.clone());

    // First run: the CEO and the brief box, no main panel.
    pump_until(&mut vcx, "the brief box is drawn", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    assert!(
        bounds(&mut vcx, "tab-Team").is_none(),
        "the main panel is hidden"
    );

    // Two files chosen through the platform's file chooser, uploaded through the protocol.
    let files = run.daemon.dir.path().join("attach");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("sketch.png"), PNG).unwrap();
    std::fs::write(files.join("notes.md"), "# Notes\nForest greens, maybe.\n").unwrap();
    click(&mut vcx, "attach");
    assert!(run.cx.did_prompt_for_paths(), "the file chooser opened");
    let chosen = vec![files.join("sketch.png"), files.join("notes.md")];
    run.cx.simulate_path_prompt_response(move |_| Some(chosen));
    let composer = vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .chat
            .read(cx)
            .composer()
            .clone()
    });
    pump_until(&mut vcx, "both files are uploaded", |vcx| {
        vcx.read(|cx| composer.read(cx).attachments().len() == 2)
    });
    let attached: Vec<_> = vcx.read(|cx| {
        composer
            .read(cx)
            .attachments()
            .iter()
            .map(|f| (f.name.clone(), f.media_type.clone(), f.size))
            .collect()
    });
    assert_eq!(
        attached,
        [
            (
                "sketch.png".to_owned(),
                "image/png".to_owned(),
                PNG.len() as u64
            ),
            ("notes.md".to_owned(), "text/markdown".to_owned(), 30),
        ]
    );

    // The brief, typed and sent with Enter.
    click(&mut vcx, "composer-input");
    vcx.simulate_input(
        "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
    );
    vcx.simulate_keystrokes("enter");
    pump(&mut vcx);
    run.rt
        .block_on(client.wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            p.messages()
                .find(|m| m.message.route == Route::BuilderToCeo)
                .map(|m| m.message.clone())
        }))
        .map(|brief| {
            assert!(
                brief.body.starts_with("Launch Fernleaf\nA landing page"),
                "{}",
                brief.body
            );
            assert_eq!(brief.attachments.len(), 2, "the files went with the brief");
        })
        .expect("the brief reached the daemon");
    run.settle(&client, project);
    pump_until(&mut vcx, "the main panel opens", |vcx| {
        bounds(vcx, "tab-Team").is_some()
    });

    // The Team tab: the three leads the CEO formed, each row as the state has it.
    let team = team(window, &vcx, project);
    rows_follow_state(&mut vcx, &team, &client, project);
    let shown = rows(&vcx, &team);
    assert_eq!(
        names(&shown),
        ["Marketing", "Product", "Tech"].map(String::from).into()
    );
    let p = state(&client, project);
    let leads: BTreeSet<_> = p
        .project()
        .unwrap()
        .team
        .members()
        .filter(|(_, pos)| matches!(pos, Position::Lead { .. }))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(shown.iter().map(|r| r.0).collect::<BTreeSet<_>>(), leads);
    let range = |min, max| EffortRange::new(min, max).unwrap();
    let by_name: BTreeMap<_, _> = shown.iter().map(|(_, m, _)| (m.name.as_str(), m)).collect();
    assert_eq!(by_name["Tech"].range, range(Effort::Medium, Effort::Max));
    assert_eq!(by_name["Product"].range, range(Effort::Low, Effort::High));
    assert_eq!(
        by_name["Marketing"].range,
        range(Effort::Low, Effort::Medium)
    );
    // Each row's task is its agent's current task; Tech runs two agents and Product one.
    for (agent, m, _) in &shown {
        let doing = p.current_task(*agent).map(|t| t.task.title.clone());
        if let Some(title) = doing {
            assert_eq!(m.doing, title);
        }
        assert_eq!(m.agents, p.children(*agent).len());
    }
    assert_eq!((by_name["Tech"].agents, by_name["Product"].agents), (2, 1));
    assert_eq!(by_name["Product"].doing, "Write the PRD");
    assert_eq!(by_name["Product"].progress, Some(50));
    assert!(
        by_name["Product"]
            .driver
            .as_deref()
            .is_some_and(|d| d.starts_with("fake-fernleaf · fake-brief · "))
    );

    // Tabs: the BRD and the tasks changed while Team was open, so their tabs show a dot until
    // opened.
    let main = main_panel(window, &vcx, project);
    let unseen = vcx.read(|cx| main.read(cx).unseen().clone());
    assert!(
        unseen.contains(&Tab::Specs) && unseen.contains(&Tab::Tasks),
        "{unseen:?}"
    );
    assert!(!unseen.contains(&Tab::Team));
    click(&mut vcx, "tab-Specs");
    let (open, unseen) = vcx.read(|cx| (main.read(cx).open_tab(), main.read(cx).unseen().clone()));
    assert_eq!(open, Tab::Specs);
    assert!(
        !unseen.contains(&Tab::Specs) && unseen.contains(&Tab::Tasks),
        "{unseen:?}"
    );
    click(&mut vcx, "tab-Team");

    // The drivers list: every live driver, as the state has it.
    click(&mut vcx, "listing-Drivers");
    assert_eq!(vcx.read(|cx| team.read(cx).listing()), Listing::Drivers);
    let p = state(&client, project);
    let daemon = client.state().daemon;
    let live: Vec<_> = p.live_drivers().map(|d| d.driver.id).collect();
    assert!(!live.is_empty());
    pump_until(&mut vcx, "the driver rows show the state", |vcx| {
        let drawn: Vec<_> = vcx.read(|cx| {
            team.read(cx)
                .driver_rows()
                .iter()
                .map(|r| r.read(cx).shown().cloned())
                .collect()
        });
        let want: Vec<_> = live
            .iter()
            .map(|d| DriverModel::read(&p, &daemon, *d))
            .collect();
        drawn == want
    });
    let first = vcx.read(|cx| {
        team.read(cx).driver_rows()[0]
            .read(cx)
            .shown()
            .cloned()
            .unwrap()
    });
    assert_eq!(first.0[1], "fake-fernleaf");
    assert_eq!(first.0[2], "fake-brief");
    click(&mut vcx, "listing-Agents");

    // The builder's answer to the CEO's question gives Marketing its first task. Marketing's row
    // is redrawn; Tech's and Product's are left alone. The answer goes from another client, so no
    // input to this window (a click or a key redraws the whole window) falls inside the count.
    let before: BTreeMap<_, _> = rows(&vcx, &team)
        .into_iter()
        .map(|(a, m, n)| (m.name, (a, n)))
        .collect();
    let question = state(&client, project)
        .open_questions()
        .next()
        .expect("a question")
        .message
        .id;
    run.rt
        .block_on(client.send_chat(project, "Forest greens.", vec![], Some(question)))
        .expect("the answer reached the daemon");
    run.settle(&client, project);
    rows_follow_state(&mut vcx, &team, &client, project);
    let after: BTreeMap<_, _> = rows(&vcx, &team)
        .into_iter()
        .map(|(a, m, n)| (m.name, (a, n)))
        .collect();
    assert!(
        after["Marketing"].1 > before["Marketing"].1,
        "{before:?} {after:?}"
    );
    assert_eq!(after["Tech"].1, before["Tech"].1, "Tech's row was redrawn");
    assert_eq!(
        after["Product"].1, before["Product"].1,
        "Product's row was redrawn"
    );

    // Opening agents: Tech runs Backend and Frontend; Backend runs Migrations; Migrations runs
    // none, so its live output takes the place of rows. The trail leads back to the CEO.
    let p = state(&client, project);
    click(&mut vcx, "agent-Tech");
    assert_eq!(vcx.read(|cx| team.read(cx).focus()), Some(role(&p, "Tech")));
    assert_eq!(
        names(&rows(&vcx, &team)),
        ["Backend", "Frontend"].map(String::from).into()
    );
    assert!(bounds(&mut vcx, "crumb-1").is_some() && bounds(&mut vcx, "crumb-2").is_none());
    click(&mut vcx, "agent-Backend");
    assert_eq!(
        names(&rows(&vcx, &team)),
        ["Migrations"].map(String::from).into()
    );
    assert_eq!(
        p.trail(role(&p, "Backend")),
        [role(&p, "CEO"), role(&p, "Tech"), role(&p, "Backend")]
    );
    assert!(bounds(&mut vcx, "crumb-2").is_some());
    click(&mut vcx, "agent-Migrations");
    let (rows_now, output) = vcx.read(|cx| {
        let t = team.read(cx);
        (t.rows().len(), t.output().map(|o| o.read(cx).agent()))
    });
    assert_eq!((rows_now, output), (0, Some(role(&p, "Migrations"))));
    assert!(bounds(&mut vcx, "crumb-3").is_some());
    // Back along the trail to Tech, then to the CEO.
    click(&mut vcx, "crumb-1");
    assert_eq!(vcx.read(|cx| team.read(cx).focus()), Some(role(&p, "Tech")));
    click(&mut vcx, "crumb-0");
    assert_eq!(vcx.read(|cx| team.read(cx).focus()), None);
    assert_eq!(
        names(&rows(&vcx, &team)),
        ["Marketing", "Product", "Tech"].map(String::from).into()
    );
}

#[test]
fn a_new_client_shows_the_same_project() {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
    run.rt
        .block_on(client.send_brief(
            project,
            "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
            vec![],
        ))
        .expect("the brief is sent");
    run.settle(&client, project);
    let (window, mut vcx) = run.open(client.clone());
    let team1 = team(window, &vcx, project);
    pump_until(&mut vcx, "the team is drawn", |vcx| {
        rows(vcx, &team1).len() == 3
    });
    rows_follow_state(&mut vcx, &team1, &client, project);
    let seen: Vec<_> = rows(&vcx, &team1)
        .into_iter()
        .map(|(a, m, _)| (a, m))
        .collect();
    let sidebar = |window: WindowHandle<Shell>, vcx: &VisualTestContext| {
        vcx.read(|cx| {
            let shell = window.read(cx).unwrap();
            shell.sidebar().read(cx).lines(cx)
        })
    };
    let listed = sidebar(window, &vcx);

    // Everything the app held goes: the window, its views and the client.
    vcx.update(|window, _| window.remove_window());
    pump(&mut vcx);
    drop(client);

    let client = run.client();
    let (window, mut vcx) = run.open(client.clone());
    pump_until(&mut vcx, "the project is selected", |vcx| {
        vcx.read(|cx| window.read(cx).unwrap().session().read(cx).selected()) == Some(project)
    });
    assert_eq!(sidebar(window, &vcx), listed);
    let team2 = team(window, &vcx, project);
    pump_until(&mut vcx, "the same team is drawn", |vcx| {
        rows(vcx, &team2)
            .into_iter()
            .map(|(a, m, _)| (a, m))
            .collect::<Vec<_>>()
            == seen
    });
    assert!(bounds(&mut vcx, "tab-Team").is_some(), "past the first run");
}

#[test]
fn a_project_is_created_from_the_sidebar() {
    let mut run = Run::start();
    let client = run.client();
    let (window, mut vcx) = run.open(client.clone());
    pump(&mut vcx);
    click(&mut vcx, "new-project");
    // The name field has the focus; the directory is the next field.
    vcx.simulate_input("Todo");
    let path = run.daemon.project_dir("todo");
    let sidebar = vcx.read(|cx| window.read(cx).unwrap().sidebar().clone());
    click(&mut vcx, "new-project-path");
    vcx.simulate_input(&path);
    vcx.simulate_keystrokes("enter");
    let project = run
        .rt
        .block_on(client.wait_for(WAIT, |s| {
            s.daemon
                .projects()
                .find(|(_, e)| e.name == "Todo" && e.path == path)
                .map(|(id, _)| id)
        }))
        .expect("the project reached the daemon");
    pump_until(&mut vcx, "the new project is listed and selected", |vcx| {
        let lines = vcx.read(|cx| sidebar.read(cx).lines(cx));
        let selected = vcx.read(|cx| window.read(cx).unwrap().session().read(cx).selected());
        lines
            .iter()
            .any(|l| l.id == project && l.agents == 1 && !l.live)
            && selected == Some(project)
    });
    // A new project is in its first run.
    pump_until(&mut vcx, "the brief box is drawn", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    assert!(bounds(&mut vcx, "tab-Team").is_none());
}
