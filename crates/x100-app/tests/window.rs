//! The app's window against a real daemon running the Fernleaf script, without a display: with no
//! project, the CEO and the box for the first message, which creates the project as it is sent;
//! a new project shown as "Untitled" until its CEO names it; "+ New project" and the
//! existing-folder control; the Team tab following the state, opening agents, the drivers list, the
//! tabs' dots, rows redrawn only when their own agent changes, and a new client seeing the same.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::*;
use gpui::{Entity, VisualTestContext, WindowHandle};
use x100_app::platform::FileChooser;
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
fn the_first_message_creates_the_project_and_the_team_tab_follows_the_organisation() {
    let mut run = Run::start();
    let client = run.client();
    run.long_waits(&client);
    let (window, mut vcx) = run.open(client.clone());

    // No project: the CEO and the box for the first message straight away, no form, no panel.
    pump_until(&mut vcx, "the message box is drawn", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });
    assert!(
        bounds(&mut vcx, "tab-Team").is_none(),
        "the main panel is hidden"
    );
    assert!(bounds(&mut vcx, "new-project-path").is_none(), "no form");

    // Two files chosen through the platform's file chooser, which "+" opens on a desktop that has
    // one, kept until the project exists.
    let files = run.daemon.dir.path().join("attach");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("sketch.png"), PNG).unwrap();
    std::fs::write(files.join("notes.md"), "# Notes\nForest greens, maybe.\n").unwrap();
    run.cx.update(|cx| cx.set_global(FileChooser(true)));
    click(&mut vcx, "add-files");
    assert!(run.cx.did_prompt_for_paths(), "the file chooser opened");
    assert!(
        bounds(&mut vcx, "attach-path").is_none(),
        "and no path field"
    );
    let chosen = vec![files.join("sketch.png"), files.join("notes.md")];
    run.cx
        .simulate_path_prompt_response(move |_| Some(chosen.clone()));
    let composer = vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .start()
            .expect("the start of a new project")
            .read(cx)
            .composer()
            .clone()
    });
    pump_until(&mut vcx, "both files are chosen", |vcx| {
        vcx.read(|cx| composer.read(cx).pending().len() == 2)
    });
    assert!(
        client.with_state(|s| s.projects.is_empty()),
        "nothing is created before the first message"
    );

    // The first message, typed and sent with Enter, creates the project.
    click(&mut vcx, "composer-input");
    vcx.simulate_input(
        "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
    );
    vcx.simulate_keystrokes("enter");
    pump(&mut vcx);
    let (project, first) = pump_for(
        &mut vcx,
        &client,
        ("the first message is sent", WAIT),
        |s| {
            s.projects.iter().find_map(|(id, p)| {
                p.messages()
                    .find(|m| m.message.route == Route::BuilderToCeo)
                    .map(|m| (*id, m.message.clone()))
            })
        },
    );
    assert!(
        first.body.starts_with("Launch Fernleaf\nA landing page"),
        "{}",
        first.body
    );
    let p = state(&client, project);
    let sent: Vec<_> = first
        .attachments
        .iter()
        .map(|f| {
            let f = &p.file(*f).unwrap().file;
            (f.name.clone(), f.media_type.clone(), f.size)
        })
        .collect();
    assert_eq!(
        sent,
        [
            (
                "sketch.png".to_owned(),
                "image/png".to_owned(),
                PNG.len() as u64
            ),
            ("notes.md".to_owned(), "text/markdown".to_owned(), 30),
        ],
        "the files went with it"
    );
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

    // The top level: the CEO large over its leads, no head in the corner.
    assert!(!vcx.read(|cx| team.read(cx).drilled()));
    assert!(bounds(&mut vcx, "drill-head").is_none());

    // Opening agents: Tech runs Backend and Frontend; Backend runs Migrations; Migrations runs
    // none, so its live output takes the place of rows. Each is a drilldown: its head and name in
    // the top row's corner, with the agents above it back to the CEO beside them.
    let p = state(&client, project);
    click(&mut vcx, "agent-Tech");
    assert_eq!(vcx.read(|cx| team.read(cx).focus()), Some(role(&p, "Tech")));
    assert_eq!(
        names(&rows(&vcx, &team)),
        ["Backend", "Frontend"].map(String::from).into()
    );
    assert_eq!(corner_name(&vcx, &team), "Tech");
    assert!(bounds(&mut vcx, "crumb-0").is_some() && bounds(&mut vcx, "crumb-1").is_none());
    // Its switch between its agents and its own output is in the top row, beside the listing's.
    let row_of = |vcx: &mut VisualTestContext, selector| bounds(vcx, selector).unwrap().center().y;
    let top = row_of(&mut vcx, "drill-head");
    for selector in ["drill-name", "crumb-0", "shown-Output", "listing-Drivers"] {
        assert!(
            (row_of(&mut vcx, selector) - top).abs() < gpui::px(4.),
            "{selector} is not in the top row"
        );
    }
    click(&mut vcx, "agent-Backend");
    assert_eq!(
        names(&rows(&vcx, &team)),
        ["Migrations"].map(String::from).into()
    );
    assert_eq!(
        p.trail(role(&p, "Backend")),
        [role(&p, "CEO"), role(&p, "Tech"), role(&p, "Backend")]
    );
    assert!(bounds(&mut vcx, "crumb-1").is_some() && bounds(&mut vcx, "crumb-2").is_none());
    click(&mut vcx, "agent-Migrations");
    let (rows_now, output) = vcx.read(|cx| {
        let t = team.read(cx);
        (t.rows().len(), t.output().map(|o| o.read(cx).agent()))
    });
    assert_eq!((rows_now, output), (0, Some(role(&p, "Migrations"))));
    assert_eq!(corner_name(&vcx, &team), "Migrations");
    assert!(bounds(&mut vcx, "crumb-2").is_some());
    // Everything under the top row is its output: no big head, task line or driver pill.
    assert_drilldown_fills_the_view(&mut vcx);
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

/// The name in a drilldown's corner.
fn corner_name(vcx: &VisualTestContext, team: &Entity<TeamView>) -> String {
    vcx.read(|cx| {
        let header = team.read(cx).header().expect("the agent opened");
        header.read(cx).shown().expect("its model").name.clone()
    })
}

/// The head and name sit in the top row's left corner, and the output fills everything below
/// that row, to the bottom of the window.
fn assert_drilldown_fills_the_view(vcx: &mut VisualTestContext) {
    let head = bounds(vcx, "drill-head").expect("the head in the corner");
    let name = bounds(vcx, "drill-name").expect("the name beside it");
    let output = bounds(vcx, "drill-output").expect("the output");
    let team = bounds(vcx, "listing-Agents").expect("the top row's switch");
    assert!(
        head.size.width <= gpui::px(24.),
        "the head is small: {head:?}"
    );
    assert!(name.left() > head.right(), "{name:?} {head:?}");
    assert!(
        head.left() < output.left() + gpui::px(24.),
        "{head:?} {output:?}"
    );
    assert!(head.bottom() <= output.top() && team.bottom() <= output.top());
    assert!(
        output.top() - head.top() < gpui::px(56.),
        "{head:?} {output:?}"
    );
    let window = vcx.update(|window, _| window.viewport_size());
    assert!(
        window.height - output.bottom() < gpui::px(2.),
        "{output:?} in {window:?}"
    );
    assert!(output.size.height > gpui::px(400.), "{output:?}");
}

#[test]
fn a_new_client_shows_the_same_project() {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
    run.rt
        .block_on(client.send_chat(
            project,
            "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
            vec![],
            None,
        ))
        .expect("the first message is sent");
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

/// What the sidebar lists, by name, and the window's title.
fn listed(window: WindowHandle<Shell>, vcx: &VisualTestContext) -> (Vec<String>, String) {
    vcx.read(|cx| {
        let shell = window.read(cx).unwrap();
        let lines = shell.sidebar().read(cx).lines(cx);
        (
            lines.into_iter().map(|l| l.name).collect(),
            shell.title().to_owned(),
        )
    })
}

fn send(vcx: &mut VisualTestContext, text: &str) {
    click(vcx, "composer-input");
    vcx.simulate_input(text);
    vcx.simulate_keystrokes("enter");
    pump(vcx);
}

#[test]
fn a_new_project_is_named_when_its_ceo_sets_it_up_and_another_waits_for_its_message() {
    let mut run = Run::start();
    let client = run.client();
    run.long_waits(&client);
    let (window, mut vcx) = run.open(client.clone());
    pump_until(&mut vcx, "the message box is drawn", |vcx| {
        bounds(vcx, "composer-input").is_some()
    });

    // Under WSL there is no file chooser, so "+" opens a field for a file's path in its place,
    // and Enter adds the file.
    run.cx.update(|cx| cx.set_global(FileChooser(false)));
    let note = run.daemon.dir.path().join("note.md");
    std::fs::write(&note, "# A note\n").unwrap();
    let composer = vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .start()
            .expect("the start of a new project")
            .read(cx)
            .composer()
            .clone()
    });
    assert!(bounds(&mut vcx, "attach-path").is_none());
    click(&mut vcx, "add-files");
    assert!(
        bounds(&mut vcx, "attach-path").is_some(),
        "the path field opened"
    );
    assert!(
        !run.cx.did_prompt_for_paths(),
        "no file chooser was asked for"
    );
    vcx.simulate_input(&note.display().to_string());
    vcx.simulate_keystrokes("enter");
    pump_until(&mut vcx, "the file is added", |vcx| {
        vcx.read(|cx| composer.read(cx).pending() == [note.clone()])
    });
    assert!(
        bounds(&mut vcx, "attach-path").is_none(),
        "and the field closed"
    );
    assert!(!vcx.read(|cx| composer.read(cx).typing_path()));

    // "hi" creates a project, shown as "Untitled" while the CEO only answers: not "New project",
    // which is what the control under the list says.
    send(&mut vcx, "hi there");
    let project = pump_for(&mut vcx, &client, ("the CEO answers", WAIT), |s| {
        s.projects.iter().find_map(|(id, p)| {
            p.messages()
                .any(|m| m.message.route == Route::CeoToBuilder)
                .then_some(*id)
        })
    });
    pump_until(&mut vcx, "the new project is listed and selected", |vcx| {
        let selected = vcx.read(|cx| window.read(cx).unwrap().session().read(cx).selected());
        selected == Some(project)
            && listed(window, vcx) == (vec!["Untitled".into()], "x100 \u{b7} Untitled".into())
    });
    assert!(!state(&client, project).project().unwrap().is_set_up());

    // A CEO with no leads has nothing else to show: the Team tab opens straight to its drilldown.
    pump_until(&mut vcx, "the main panel opens", |vcx| {
        bounds(vcx, "tab-Team").is_some()
    });
    let team = team(window, &vcx, project);
    let ceo = state(&client, project).project().unwrap().ceo;
    pump_until(&mut vcx, "the CEO's drilldown is drawn", |vcx| {
        bounds(vcx, "drill-head").is_some()
    });
    let (focus, drilled, output) = vcx.read(|cx| {
        let t = team.read(cx);
        (
            t.focus(),
            t.drilled(),
            t.output().map(|o| o.read(cx).agent()),
        )
    });
    assert_eq!((focus, drilled, output), (None, true, Some(ceo)));
    assert_eq!(corner_name(&vcx, &team), "CEO");
    assert!(
        bounds(&mut vcx, "crumb-0").is_none(),
        "nothing above the CEO"
    );
    assert_drilldown_fills_the_view(&mut vcx);

    // The brief: the CEO sets the project up, and the name follows everywhere.
    send(
        &mut vcx,
        "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
    );
    pump_until(&mut vcx, "the project takes its name", |vcx| {
        listed(window, vcx) == (vec!["Fernleaf".into()], "x100 \u{b7} Fernleaf".into())
    });

    // The CEO's own output beside its leads: its skills, which run to pages, stay under the
    // output in a part of their own and never draw over it.
    pump_until(&mut vcx, "the CEO's output can be shown", |vcx| {
        bounds(vcx, "shown-Output").is_some()
    });
    click(&mut vcx, "shown-Output");
    pump_until(&mut vcx, "the CEO's skills are listed", |vcx| {
        vcx.read(|cx| {
            team.read(cx).reflection().is_some_and(|r| {
                r.read(cx)
                    .listed(cx)
                    .skills
                    .iter()
                    .any(|s| s.body.is_some())
            })
        })
    });
    let own = bounds(&mut vcx, "drill-own-output").expect("the output");
    let skills = bounds(&mut vcx, "drill-reflection").expect("the skills");
    let column = bounds(&mut vcx, "drill-output").expect("the column");
    assert!(own.bottom() <= skills.top(), "{own:?} {skills:?}");
    assert!(
        own.size.height * 2. >= column.size.height,
        "the output keeps most of the room: {own:?} in {column:?}"
    );

    // "+ New project" opens the start of another, which exists only once its message is sent.
    click(&mut vcx, "new-project");
    pump_until(&mut vcx, "the start of a new project", |vcx| {
        let selected = vcx.read(|cx| window.read(cx).unwrap().session().read(cx).selected());
        selected.is_none() && bounds(vcx, "composer-input").is_some()
    });
    assert_eq!(
        listed(window, &vcx),
        (vec!["Fernleaf".into()], "x100".into())
    );
    assert_eq!(
        client.with_state(|s| s.projects.len()),
        1,
        "nothing created"
    );

    // On an existing folder: named under the box, then the first message starts it there.
    let shop = run.daemon.dir.path().join("shop");
    std::fs::create_dir_all(&shop).unwrap();
    std::fs::write(shop.join("README.md"), "# Shop\n").unwrap();
    click(&mut vcx, "open-folder");
    vcx.simulate_input(&shop.display().to_string());
    vcx.simulate_keystrokes("enter");
    pump_until(&mut vcx, "the folder is named", |vcx| {
        bounds(vcx, "no-folder").is_some()
    });
    send(&mut vcx, "hi there");
    let started = pump_for(&mut vcx, &client, ("a project on the folder", WAIT), |s| {
        s.projects
            .iter()
            .find(|(id, _)| **id != project)
            .and_then(|(id, p)| Some((*id, p.project()?.code_dir.clone()?)))
    });
    assert_eq!(started.1, shop.canonicalize().unwrap());
    pump_until(&mut vcx, "it is listed as new and selected", |vcx| {
        let selected = vcx.read(|cx| window.read(cx).unwrap().session().read(cx).selected());
        selected == Some(started.0) && listed(window, vcx).0.len() == 2
    });
    assert!(listed(window, &vcx).0.contains(&"Untitled".to_owned()));
}
