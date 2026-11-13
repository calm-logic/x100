//! An agent's look against a real daemon running the Fernleaf script on the fake clock: the Team
//! tree draws every agent's outline as its glyph in its colour, as the daemon recorded them, and
//! `l` on an agent's page changes both through the daemon, after which the tree shows the new look.

mod common;

use common::*;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;
use x100_core::appearance::{Appearance, Hue, Outline};
use x100_core::fold::ProjectState;
use x100_tui::app::Tab;
use x100_tui::look::{self, Colours};
use x100_tui::page::Target;
use x100_tui::remember::Remember;

/// The glyph drawn before `role` in the tree, "● ⬢ Product", and its colour.
fn face_of(buffer: &Buffer, role: &str) -> Option<(String, Option<Color>)> {
    let width = buffer.area.width as usize;
    buffer.content().chunks(width).find_map(|row| {
        let at = row.windows(role.chars().count() + 2).position(|w| {
            w[0].symbol() != " "
                && w[1].symbol() == " "
                && w[2..].iter().map(|c| c.symbol()).collect::<String>() == role
        })?;
        let cell = &row[at];
        Some((
            cell.symbol().to_owned(),
            Some(cell.fg).filter(|c| *c != Color::Reset),
        ))
    })
}

#[test]
fn the_tree_draws_each_agents_look_and_l_changes_it_through_the_daemon() {
    let run = Run::start();
    let project = run.fernleaf_project();
    run.rt
        .block_on(run.client.send_chat(project, BRIEF, vec![], None))
        .expect("the first message is sent");
    run.settle(project);
    let mut screen = run.tui(Remember::none());
    screen.tui.app.colours = Colours::True;
    screen.tui.app.open_project(project);
    screen.tui.app.set_tab(Tab::Team);
    let state = || -> ProjectState { run.client.project(project).expect("the project") };

    let buffer = screen.buffer();
    let agents: Vec<_> = state()
        .agents()
        .map(|a| (a.role.clone(), a.appearance))
        .collect();
    assert!(agents.len() > 3, "the team is formed");
    for (role, appearance) in &agents {
        let shown = face_of(&buffer, role).unwrap_or_else(|| panic!("{role} is not in the tree"));
        assert_eq!(
            shown,
            (
                look::glyph(appearance.outline).to_owned(),
                Colours::True.of(appearance.hue)
            ),
            "{role}"
        );
    }

    // Product's page: l, then an outline and a colour by number.
    let product = state()
        .agents()
        .find(|a| a.role == "Product")
        .map(|a| a.id)
        .expect("Product");
    assert!(screen.select(&Target::Agent(product)));
    screen.press(KeyCode::Enter);
    screen.press(KeyCode::Char('l'));
    let asked = screen.text();
    assert!(
        asked.contains("3 ⬢ Hexagon"),
        "the outlines are offered:\n{asked}"
    );
    screen.type_text("3");
    screen.press(KeyCode::Enter);
    let asked = screen.text();
    assert!(
        asked.contains("5 Teal"),
        "the colours are offered:\n{asked}"
    );
    screen.type_text("5");
    screen.press(KeyCode::Enter);
    let want = Appearance {
        outline: Outline::Hexagon,
        hue: Some(Hue::Teal),
    };
    let client = run.client.clone();
    screen.wait_for(&run.rt, "the daemon has Product's new look", |_| {
        client
            .project(project)
            .and_then(|p| p.agent(product).map(|a| a.appearance))
            == Some(want)
    });
    let text = screen.wait_for(&run.rt, "the look is confirmed", |t| {
        t.contains("It is a teal hexagon now")
    });
    assert!(text.contains("Product"), "{text}");

    screen.press(KeyCode::Esc);
    let buffer = screen.buffer();
    assert_eq!(
        face_of(&buffer, "Product"),
        Some(("⬢".to_owned(), Colours::True.of(Some(Hue::Teal))))
    );
    // Enter on both keeps the look as it is.
    assert!(screen.select(&Target::Agent(product)));
    screen.press(KeyCode::Enter);
    screen.press(KeyCode::Char('l'));
    screen.press(KeyCode::Enter);
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "the kept look is confirmed", |t| {
        t.contains("It is a teal hexagon now")
    });
    assert_eq!(
        run.client
            .project(project)
            .and_then(|p| p.agent(product).map(|a| a.appearance)),
        Some(want)
    );
}
