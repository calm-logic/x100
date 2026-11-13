//! The first run, the project list, and quitting and reopening against a real daemon: the box for
//! the first message is the whole of the first screen, sending it makes the project, and the next
//! run opens on the same project and tab.

mod common;

use common::*;
use ratatui::crossterm::event::KeyCode;
use x100_tui::app::{Tab, Where};
use x100_tui::remember::Remember;

fn type_brief(screen: &mut Screen) {
    screen.type_text("Launch Fernleaf");
    screen.ctrl('j');
    screen.type_text("A landing page for Fernleaf, a plant shop, with a waitlist.");
}

#[test]
fn the_first_message_makes_the_project_and_the_next_run_reopens_it() {
    let run = Run::start();
    run.long_waits();
    let remember = || Remember::beside(&run.daemon.socket());

    let mut first = run.tui(remember());
    let text = first.text();
    assert!(matches!(first.tui.app.at, Where::New), "{text}");
    assert!(
        text.contains("Tell the CEO what you want to build."),
        "{text}"
    );
    type_brief(&mut first);
    first.press(KeyCode::Enter);
    let text = first.wait_for(&run.rt, "the project is open on its chat", |s| {
        s.contains("x100 · Fernleaf") && s.contains("Setting it up now.")
    });
    assert!(text.contains("You    Launch Fernleaf"), "{text}");
    let project = first.tui.app.project_ui().expect("a project is open").id;
    assert_eq!(run.client.state().projects.len(), 1);
    // Leave on another tab: the next run opens there.
    first.press(KeyCode::Tab);
    first.press(KeyCode::Char('7'));
    assert_eq!(first.tui.app.project_ui().unwrap().tab, Tab::Budget);
    drop(first);

    let mut second = run.tui(remember());
    let text = second.wait_for(&run.rt, "the same project", |s| {
        s.contains("x100 · Fernleaf")
    });
    let ui = second.tui.app.project_ui().expect("it opens on a project");
    assert_eq!((ui.id, ui.tab), (project, Tab::Budget), "{text}");
    assert!(text.contains("spent of this project's"), "{text}");

    // The list shows it with its agent count, and Enter opens it again on the chat.
    second.press(KeyCode::Char('p'));
    let text = second.text();
    let agents = run.client.project(project).unwrap().agent_count();
    let line = text
        .lines()
        .find(|l| l.contains("Fernleaf"))
        .expect("listed");
    assert!(line.contains(&x100_client::words::agents(agents)), "{line}");
    second.press(KeyCode::Enter);
    assert_eq!(second.tui.app.project_ui().unwrap().tab, Tab::Chat);

    // `n` is a second project: no name until its CEO sets it up.
    second.ctrl('o');
    second.press(KeyCode::Char('n'));
    assert!(
        second
            .text()
            .contains("Tell the CEO what you want to build.")
    );
}

#[test]
fn a_project_can_start_on_an_existing_folder() {
    let run = Run::start();
    run.long_waits();
    let folder = run.daemon.dir.path().join("existing");
    std::fs::create_dir_all(&folder).unwrap();
    let mut screen = run.tui(Remember::none());
    screen.ctrl('d');
    assert!(screen.text().contains("Start on an existing folder"));
    screen.type_text(folder.to_str().unwrap());
    screen.press(KeyCode::Enter);
    assert!(screen.text().contains("Starts on the folder"));
    screen.type_text("Work on what is here");
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "the project is open", |s| s.contains("1 Chat"));
    let project = screen.tui.app.project_ui().unwrap().id;
    let state = run
        .rt
        .block_on(
            run.client
                .wait_for(WAIT, |s| s.projects.get(&project).cloned()),
        )
        .expect("the client holds it");
    assert_eq!(
        state.project().unwrap().code_dir.as_deref(),
        Some(folder.as_path())
    );
}
