//! An agent's page against a real daemon running the Fernleaf script on the fake clock: opening an
//! agent three levels down shows its output as it arrives, the output carries on in the same view
//! across a change of driver with the change marked, and an agent that runs agents shows its own
//! output one switch away.

mod common;

use common::*;
use ratatui::crossterm::event::KeyCode;
use x100_core::fold::ProjectState;
use x100_core::id::AgentId;
use x100_core::time::Timestamp;
use x100_proto::SetClock;
use x100_tui::app::{AgentMode, Tab};
use x100_tui::page::Target;
use x100_tui::remember::Remember;

const HOUR: std::time::Duration = std::time::Duration::from_secs(3600);

struct Open {
    run: Run,
    project: x100_core::id::ProjectId,
    screen: Screen,
}

/// The interface open on the Team tab of a new project, whose brief is then sent from another
/// client so the script runs while the interface is open.
fn started() -> Open {
    let run = Run::start();
    let project = run.fernleaf_project();
    let mut screen = run.tui(Remember::none());
    screen.tui.app.open_project(project);
    screen.tui.app.set_tab(Tab::Team);
    run.rt
        .block_on(run.client.send_chat(project, BRIEF, vec![], None))
        .expect("the first message is sent");
    Open {
        run,
        project,
        screen,
    }
}

impl Open {
    fn state(&self) -> ProjectState {
        self.run.client.project(self.project).expect("the project")
    }

    fn agent(&self, role: &str) -> Option<AgentId> {
        self.state().agents().find(|a| a.role == role).map(|a| a.id)
    }

    fn wait(&mut self, what: &str, done: impl FnMut(&str) -> bool) -> String {
        self.screen.wait_for(&self.run.rt, what, done)
    }

    /// Selects the row of `role` as soon as it is listed, and opens it.
    fn open(&mut self, role: &str) {
        let until = std::time::Instant::now() + WAIT;
        loop {
            self.screen.text();
            if let Some(id) = self.agent(role) {
                let listed = (0..30).find_map(|_| {
                    if self.screen.tui.app.selected() == Some(Target::Agent(id)) {
                        return Some(());
                    }
                    self.screen.press(KeyCode::Down);
                    None
                });
                if listed.is_some() {
                    self.screen.press(KeyCode::Enter);
                    return;
                }
                // Not listed yet where this page is: start from the top and look again.
                self.screen.press(KeyCode::Home);
            }
            assert!(std::time::Instant::now() < until, "{role} was never listed");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn output(&mut self) -> String {
        self.screen.text()
    }
}

fn in_order(log: &str, parts: &[&str]) {
    let mut at = 0;
    for part in parts {
        let found = log[at..]
            .find(part)
            .unwrap_or_else(|| panic!("{part:?} is not where it belongs in\n{log}"));
        at += found + part.len();
    }
}

#[test]
fn an_agent_three_levels_down_shows_its_output_as_it_arrives() {
    let mut o = started();
    o.open("Tech");
    o.open("Backend");
    o.open("Migrations");
    let migrations = o.agent("Migrations").unwrap();
    let ui = o.screen.tui.app.project_ui().unwrap();
    assert_eq!(ui.agent.as_ref().unwrap().id, migrations);
    assert_eq!(ui.agent.as_ref().unwrap().mode, AgentMode::Output);

    // Each different screen seen while the script runs, until Migrations' turn has ended.
    let mut seen: Vec<String> = Vec::new();
    let text = o.wait("Migrations' turn is shown", |s| {
        if seen.last().map(String::as_str) != Some(s) {
            seen.push(s.to_owned());
        }
        s.contains("── turn ended ──")
    });
    assert!(seen.len() > 1, "the output arrived in one piece: {seen:#?}");
    shows_trail(&text, &["CEO", "Tech", "Backend", "Migrations"]);
    in_order(
        &text,
        &[
            "━━ Driver 1 started: fake-fernleaf · fake-brief · ",
            "for the task “Write the schema migrations”.",
            "started from a start pack of",
            "▸ turn started",
            "▸ write migrations/0001.sql",
            "▸ report ",
            "usage: 700 in · 200 out",
            "── turn ended ──",
        ],
    );
}

fn shows_trail(text: &str, roles: &[&str]) {
    let trail = roles.join(" > ");
    assert!(text.contains(&trail), "{trail:?} is not on\n{text}");
}

#[test]
fn the_output_carries_on_across_a_change_of_driver_with_the_change_marked() {
    let mut o = started();
    o.run.settle(o.project);
    o.open("Product");
    o.open("Research");
    let first = o.wait("Research's first turn is shown", |s| {
        s.contains("── turn ended ──")
    });

    // An hour on, the job wakes Research on a new driver. Its output carries on in the same view,
    // live, with the change marked.
    let client = o.run.client.clone();
    o.run
        .rt
        .block_on(client.request(SetClock {
            at: Timestamp::from_millis(START.as_millis() + HOUR.as_millis() as u64),
        }))
        .expect("the clock moves");
    o.run.settle(o.project);
    let after = o.wait("the second driver's report is shown", |s| {
        s.contains("Two of three price per seat.")
    });
    let marker = "━━ Driver 2 took over: fake-fernleaf · fake-brief · Low, woken by its inbox. \
                  Driver 1 ended: retired, warm too long.";
    assert!(after.contains(marker), "{after}");
    // One view: the first driver's lines are still there, above the marker, and the new
    // driver's below it.
    // Below the trail and above the footer.
    let body = |s: &str| {
        let lines: Vec<&str> = s.lines().collect();
        lines[5..lines.len() - 1].join("\n")
    };
    let start = body(&first);
    let log = body(&after);
    let start = start.trim_end();
    assert!(
        log.starts_with(start),
        "the first driver's output moved:\n{log}"
    );
    assert!(log.find(marker).unwrap() < log.find("Two of three price per seat.").unwrap());
    assert_eq!(log.matches("━━ Driver 1 started").count(), 1);
}

#[test]
fn an_agent_that_runs_agents_shows_its_own_output_one_switch_away() {
    let mut o = started();
    o.run.settle(o.project);
    o.open("Product");
    let tech = o.agent("Product").unwrap();
    let text = o.output();
    // Its page lists the agents it runs.
    assert_eq!(
        o.screen
            .tui
            .app
            .project_ui()
            .unwrap()
            .agent
            .as_ref()
            .unwrap()
            .mode,
        AgentMode::Children
    );
    shows_trail(&text, &["CEO", "Product"]);
    for child in o.state().children(tech) {
        let role = o.state().agent(child).unwrap().role.clone();
        assert!(text.contains(&role), "{role} is not on\n{text}");
    }
    // `o` switches to what Tech is working in itself.
    o.screen.press(KeyCode::Char('o'));
    let text = o.wait("Product's own output", |s| {
        s.contains("Driver 1 started") && s.contains("turn ended")
    });
    assert!(
        text.contains("▸ write_spec") && text.contains("▸ message"),
        "{text}"
    );
    // Waits and jobs, from the same state.
    o.screen.press(KeyCode::Char('w'));
    let text = o.screen.text();
    assert!(
        text.contains("WAITING FOR") && text.contains("JOBS IT SCHEDULED"),
        "{text}"
    );
    let waits: Vec<_> = o
        .state()
        .agent(tech)
        .unwrap()
        .waits
        .values()
        .cloned()
        .collect();
    assert!(!waits.is_empty());
    assert!(
        text.contains(&x100_client::words::moment(waits[0].deadline)),
        "{text}"
    );
    // Esc comes back to where the builder was.
    o.screen.press(KeyCode::Esc);
    assert!(o.screen.tui.app.project_ui().unwrap().agent.is_none());
}

#[test]
fn a_drivers_work_is_handed_on_when_the_builder_replaces_or_stops_it() {
    let mut o = started();
    o.run.settle(o.project);
    o.open("Product");
    o.open("Research");
    let research = o.agent("Research").unwrap();
    let drivers = |o: &Open| o.state().agent(research).unwrap().drivers.clone();
    let before = drivers(&o);
    assert_eq!(before.len(), 1);

    // `r` on the agent's page replaces its driver. Research is waiting on a job, so the first
    // driver ends and the next starts when the job wakes it, in the same view, with the change
    // marked and the builder's replacing named as the reason.
    let first = before[0];
    o.screen.press(KeyCode::Char('r'));
    let client = o.run.client.clone();
    let project = o.project;
    o.wait("the first driver has ended", |_| {
        !client
            .project(project)
            .unwrap()
            .driver(first)
            .unwrap()
            .state
            .is_live()
    });
    let at = Timestamp::from_millis(START.as_millis() + HOUR.as_millis() as u64);
    o.run
        .rt
        .block_on(client.request(SetClock { at }))
        .expect("the clock moves");
    o.run.settle(o.project);
    let text = o.wait("the handoff is marked", |s| {
        s.contains("Driver 2 took over")
    });
    assert!(text.contains("Two of three price per seat."), "{text}");
    let marker = text
        .lines()
        .find(|l| l.contains("Driver 2 took over"))
        .unwrap();
    assert!(marker.contains("Driver 1 ended: "), "{marker}");
    let after = drivers(&o);
    assert_eq!(after.len(), 2, "{text}");
    assert!(o.state().driver(after[1]).unwrap().state.is_live());

    // The drivers list shows it, and `s` stops it: its work goes to a third.
    o.screen.press(KeyCode::Esc);
    o.screen.press(KeyCode::Esc);
    o.screen.press(KeyCode::Esc);
    o.screen.press(KeyCode::Char('d'));
    let text = o.screen.text();
    assert!(
        text.contains("HARNESS") && text.contains("Research"),
        "{text}"
    );
    let live = after[1];
    let row = (0..12).find_map(|_| {
        if o.screen.tui.app.selected() == Some(Target::Driver(live)) {
            return Some(());
        }
        o.screen.press(KeyCode::Down);
        None
    });
    assert!(row.is_some(), "the live driver is listed");
    o.screen.press(KeyCode::Char('s'));
    let (client, project) = (o.run.client.clone(), o.project);
    o.wait("the driver has stopped", |_| {
        !client
            .project(project)
            .unwrap()
            .driver(live)
            .unwrap()
            .state
            .is_live()
    });
}

#[test]
fn notes_skills_and_trials_are_what_the_daemon_reads_for_the_agent() {
    let mut o = started();
    o.run.settle(o.project);
    o.open("Product");
    let product = o.agent("Product").unwrap();
    let (client, project) = (o.run.client.clone(), o.project);

    let notes = o
        .run
        .rt
        .block_on(client.list_memories(project, product))
        .unwrap();
    o.screen.press(KeyCode::Char('m'));
    let text = o.wait("the notes are read", |s| !s.contains("Reading its notes"));
    if notes.notes.is_empty() {
        assert!(text.contains("It has written no notes yet."), "{text}");
    }
    for n in &notes.notes {
        assert!(text.contains(&n.name.0), "{text}");
    }

    let skills = o
        .run
        .rt
        .block_on(client.list_skills(project, None))
        .unwrap();
    o.screen.press(KeyCode::Char('k'));
    let text = o.wait("the skills are read", |s| !s.contains("Reading its skills"));
    if skills.skills.is_empty() {
        assert!(text.contains("No skills yet."), "{text}");
    }
    for k in &skills.skills {
        assert!(text.contains(&k.skill.name.0), "{text}");
    }

    let trials = o.run.rt.block_on(client.list_trials(project)).unwrap();
    o.screen.press(KeyCode::Char('t'));
    let text = o.screen.text();
    let about = o.state().trials_about(product).count();
    assert_eq!(about, trials.trials.len().min(about));
    if about == 0 {
        assert!(
            text.contains("No trial is about its skills or notes."),
            "{text}"
        );
    }

    // Reflect now is a task for the CEO: the daemon refuses while the CEO holds the builder's.
    o.screen.press(KeyCode::Esc);
    o.screen.tui.app.set_tab(Tab::Settings);
    let reflect = (0..40).find_map(|_| {
        if o.screen.tui.app.selected() == Some(Target::Setting(x100_tui::page::Setting::ReflectNow))
        {
            return Some(());
        }
        o.screen.press(KeyCode::Down);
        None
    });
    assert!(reflect.is_some());
    o.screen.press(KeyCode::Enter);
    let text = o.wait("the daemon answers", |s| {
        s.contains("The CEO was asked to reflect")
            || s.lines()
                .last()
                .is_some_and(|l| l.contains("CEO") && !l.contains("Enter changes"))
    });
    let held_task = o
        .state()
        .tasks()
        .any(|t| t.task.title.to_lowercase().contains("reflect"));
    assert!(
        held_task == text.contains("The CEO was asked to reflect"),
        "{text}"
    );
}
