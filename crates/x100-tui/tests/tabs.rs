//! Every tab against a real daemon running the Fernleaf script on the fake clock: what the screen
//! shows is what the client library holds, and what the builder does from a tab reaches the daemon.

mod common;

use common::*;
use ratatui::crossterm::event::KeyCode;
use x100_client::words;
use x100_core::agent::AgentState;
use x100_core::fold::ProjectState;
use x100_core::id::ProjectId;
use x100_core::spec::SpecKind;
use x100_tui::app::Tab;
use x100_tui::remember::Remember;

const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0xf8, 0xcf, 0xc0,
    0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

struct Open {
    run: Run,
    project: ProjectId,
    screen: Screen,
}

/// The Fernleaf script run to the end with two files held, and the interface open on it.
fn script() -> Open {
    let run = Run::start();
    let project = run.fernleaf_project();
    let client = run.client.clone();
    run.rt
        .block_on(client.upload_file(project, "sketch.png", "image/png", PNG.to_vec()))
        .expect("the picture is held");
    run.rt
        .block_on(client.upload_file(project, "notes.md", "text/markdown", b"# Notes\n".to_vec()))
        .expect("the note is held");
    run.rt
        .block_on(client.send_chat(project, BRIEF, vec![], None))
        .expect("the first message is sent");
    run.settle(project);
    let mut screen = run.tui(Remember::none());
    screen.tui.app.open_project(project);
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

    fn tab(&mut self, tab: Tab) -> String {
        self.screen.tui.app.set_tab(tab);
        self.screen.text()
    }

    fn wait(&mut self, what: &str, done: impl FnMut(&str) -> bool) -> String {
        self.screen.wait_for(&self.run.rt, what, done)
    }
}

fn shows(screen: &str, what: &str) {
    assert!(screen.contains(what), "{what:?} is not on\n{screen}");
}

#[test]
fn the_chat_shows_the_conversation_and_sending_reaches_the_ceo() {
    let mut o = script();
    let text = o.tab(Tab::Chat);
    let p = o.state();
    assert!(!p.chat().is_empty());
    for entry in p.chat() {
        if let x100_core::fold::ChatItem::Message(id) = &entry.item {
            let body = &p.message(*id).unwrap().message.body;
            for line in body.lines() {
                shows(&text, line);
            }
        }
    }
    shows(&text, "Forest greens or ocean blues?  (asks you)");

    // The answer is typed in the box and goes to the CEO as a reply to its question.
    o.screen.type_text("Forest greens, please");
    o.screen.press(KeyCode::Enter);
    let question = p
        .open_questions()
        .next()
        .expect("the open question")
        .message
        .id;
    o.wait("the answer is on screen", |s| {
        s.contains("You    Forest greens, please")
    });
    let p = o.state();
    let answer = p
        .messages()
        .find(|m| m.message.body == "Forest greens, please")
        .expect("the daemon holds the answer");
    assert_eq!(answer.message.in_reply_to, Some(question));
}

#[test]
fn the_team_tab_lists_every_agent_with_what_it_is_on() {
    let mut o = script();
    let text = o.tab(Tab::Team);
    let p = o.state();
    assert_eq!(p.agents().count(), 8);
    for a in p.agents() {
        let row = p.row(a.id).expect("a row");
        let line = text
            .lines()
            .find(|l| l.contains(&format!(" {} ", a.role)) || l.trim_end().ends_with(&a.role))
            .unwrap_or_else(|| panic!("no row for {} on\n{text}", a.role));
        assert!(line.contains(words::agent_state(&a.state)), "{line}");
        if let Some(task) = row.task {
            assert!(line.contains(&task.task.title), "{line}");
        }
        if let Some(driver) = row.driver {
            assert!(line.contains(&driver.driver.model.0), "{line}");
        }
        assert!(line.contains(words::effort(row.effort.max())), "{line}");
    }
    // Depth shows as indentation: Migrations is under Backend under Tech.
    let col = |role: &str| {
        text.lines()
            .find(|l| l.contains(role))
            .and_then(|l| l.find(role))
            .unwrap()
    };
    assert!(col("Tech") < col("Backend") && col("Backend") < col("Migrations"));

    // Drivers list: every live driver with agent, model, state and cost.
    o.screen.press(KeyCode::Char('d'));
    let text = o.screen.text();
    let live: Vec<_> = p.live_drivers().collect();
    assert!(!live.is_empty());
    for d in live {
        let agent = p.agent(d.driver.agent).unwrap();
        let line = text
            .lines()
            .find(|l| l.starts_with(&agent.role) && l.contains(words::driver_state(&d.state)))
            .unwrap_or_else(|| panic!("no driver row for {} on\n{text}", agent.role));
        assert!(line.contains(&d.driver.model.0) && line.contains(&words::money(d.spent.cost)));
    }
}

#[test]
fn specs_tasks_files_budget_and_preview_show_the_held_state() {
    let mut o = script();
    let p = o.state();

    let mut text = o.tab(Tab::Specs);
    for kind in [SpecKind::Brd, SpecKind::Prd, SpecKind::Add] {
        let spec = &p.spec(kind).expect("the script wrote it").spec;
        shows(&text, words::spec_name(kind));
        shows(&text, &format!("revision {}", spec.revision));
        shows(&text, &format!("by {}", p.agent(spec.author).unwrap().role));
        for line in spec.body.lines() {
            shows(&text, line);
        }
        o.screen.press(KeyCode::Right);
        text = o.screen.text();
    }

    let text = o.tab(Tab::Tasks);
    assert_eq!(p.tasks().count(), 8);
    for t in p.tasks() {
        let line = text
            .lines()
            .find(|l| l.contains(&t.task.title))
            .unwrap_or_else(|| panic!("no row for {} on\n{text}", t.task.title));
        assert!(
            line.contains(x100_tui::views::tasks::status(&t.state)),
            "{line}"
        );
    }
    // The builder raises a task and it shows as a new idea.
    o.screen.press(KeyCode::Char('n'));
    o.screen.type_text("Check the favicon");
    o.screen.press(KeyCode::Enter);
    o.screen.type_text("It is missing on Safari");
    o.screen.press(KeyCode::Enter);
    let text = o.wait("the raised task is listed", |s| {
        s.contains("Check the favicon")
    });
    let line = text
        .lines()
        .find(|l| l.contains("Check the favicon"))
        .unwrap();
    assert!(line.contains("New idea") && line.contains("You"), "{line}");

    let text = o.tab(Tab::Files);
    for f in p.files() {
        let line = text
            .lines()
            .find(|l| l.contains(&f.file.name))
            .expect("a file row");
        assert!(
            line.contains(&words::size(f.file.size))
                && line.contains(&words::kind(&f.file.media_type))
        );
    }
    // Remove the selected file, after saying yes.
    let before = o.state().files().count();
    o.screen.press(KeyCode::Char('x'));
    shows(&o.screen.text(), "Remove ");
    o.screen.press(KeyCode::Char('y'));
    o.screen.press(KeyCode::Enter);
    let (client, project) = (o.run.client.clone(), o.project);
    o.wait("a file is gone", |_| {
        client.project(project).unwrap().files().count() == before - 1
    });

    let text = o.tab(Tab::Budget);
    let view = p.budget().unwrap();
    shows(
        &text,
        &format!(
            "{} spent of this project's {} budget",
            words::money(view.project.spent),
            words::money(view.project.budget)
        ),
    );
    for (id, line) in &view.leads {
        let row = text
            .lines()
            .find(|l| l.contains(&p.agent(*id).unwrap().role) && l.contains(" of "))
            .expect("a lead row");
        assert!(
            row.contains(&format!(
                "{} of {}",
                words::money(line.spent),
                words::money(line.budget)
            )),
            "{row}"
        );
    }
    for a in p.agents() {
        shows(&text, &a.role);
    }
    // The selected lead's budget is set from the tab.
    let lead = *view
        .leads
        .keys()
        .min_by_key(|id| p.agent(**id).map(|a| (a.joined, a.role.clone())))
        .unwrap();
    let role = p.agent(lead).unwrap().role.clone();
    o.screen.press(KeyCode::Char('e'));
    o.screen.type_text("6");
    o.screen.press(KeyCode::Enter);
    o.wait("the lead's budget changed", |s| s.contains("of $6.00"));
    let now = o.state().budget().unwrap();
    assert_eq!(
        now.leads[&lead].budget,
        x100_core::money::Money::from_dollars(6),
        "{role}"
    );

    let text = o.tab(Tab::Preview);
    assert!(p.project().unwrap().preview.is_none());
    shows(&text, "No development build is running yet");
}

#[test]
fn the_log_lists_the_projects_events_newest_first() {
    let mut o = script();
    let p = o.state();
    o.screen.tui.app.set_tab(Tab::Log);
    let newest = p.seq().unwrap();
    let text = o.wait("the log is read", |s| s.contains("Charged"));
    let events = o
        .run
        .rt
        .block_on(o.run.client.read_events_before(o.project, None, 25))
        .unwrap();
    let mut files = Default::default();
    let rows = x100_tui::logfeed::rows(
        &events.iter().rev().cloned().collect::<Vec<_>>(),
        &p,
        &o.run.client.state().daemon,
        &mut files,
    );
    assert_eq!(events[0].seq, newest);
    // The newest event is the first line, and the rest follow in order.
    let lines: Vec<&str> = text.lines().skip(2).collect();
    for (shown, row) in lines.iter().zip(rows.iter().rev()) {
        assert!(
            shown.contains(&row.who) && shown.contains(&row.at),
            "{shown} is not {row:?}"
        );
        let head: String = row.text.chars().take(60).collect();
        assert!(shown.contains(&head), "{shown} is not {row:?}");
    }
    assert!(lines.len() > 20);
}

#[test]
fn settings_change_what_the_daemon_holds_and_a_credential_value_is_never_shown() {
    let mut o = script();
    let text = o.tab(Tab::Settings);
    let p = o.state();
    let s = &p.project().unwrap().settings;
    shows(&text, words::ask_me_about(s.ask_me_about));
    shows(&text, &words::money(s.budget));
    shows(&text, &format!("{}%", s.lab_share.value()));
    for (id, pos) in p.project().unwrap().team.members() {
        if let x100_core::agent::Position::Lead { effort, budget } = pos {
            let line = text
                .lines()
                .find(|l| l.contains(&p.agent(id).unwrap().role) && l.contains("budget"))
                .expect("a lead's line");
            assert!(
                line.contains(words::effort(effort.max())) && line.contains(&words::money(*budget)),
                "{line}"
            );
        }
    }
    for h in o.run.client.state().daemon.harnesses() {
        for m in &h.models {
            let mark = if s
                .allowed
                .iter()
                .any(|a| a.model == m.model && a.harness == h.id)
            {
                "[x]"
            } else {
                "[ ]"
            };
            shows(
                &text,
                &format!(
                    "{mark} {} · {}",
                    words::harness(&o.run.client.state().daemon, h.id),
                    m.model.0
                ),
            );
        }
    }

    // Enter on the first setting moves Ask me about to the next level.
    o.screen.press(KeyCode::Enter);
    let next = words::ask_me_about(x100_core::action::AskMeAbout::MediumAndUp);
    o.wait("Ask me about changed", |s| s.contains(next));
    assert_eq!(
        o.state().project().unwrap().settings.ask_me_about,
        x100_core::action::AskMeAbout::MediumAndUp
    );

    // A credential: its value goes to the daemon and is on no frame at any point.
    let secret = "sk-test-0123456789-never-shown";
    let mut frames = vec![o.screen.text()];
    o.screen.press(KeyCode::Char('c'));
    for (answer, secret_step) in [
        ("github", false),
        ("api.github.com", false),
        ("git", false),
        (secret, true),
    ] {
        frames.push(o.screen.text());
        o.screen.type_text(answer);
        let typed = o.screen.text();
        if secret_step {
            assert!(
                !typed.contains(secret) && typed.contains("•••••"),
                "{typed}"
            );
        }
        frames.push(typed);
        o.screen.press(KeyCode::Enter);
    }
    let text = o.wait("the credential is listed", |s| s.contains("api.github.com"));
    frames.push(text.clone());
    shows(&text, "github");
    let daemon = o.run.client.state().daemon;
    let held: Vec<_> = daemon.credentials().collect();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].credential.name, "github");
    assert!(held[0].credential.scope.commands.contains("git"));
    assert!(
        frames.iter().all(|f| !f.contains(secret)),
        "the value was drawn"
    );
}

#[test]
fn tab_keys_and_dots() {
    let mut o = script();
    o.screen.press(KeyCode::Tab);
    assert_eq!(o.screen.tui.app.project_ui().unwrap().tab, Tab::Held);
    o.screen.press(KeyCode::BackTab);
    assert_eq!(o.screen.tui.app.project_ui().unwrap().tab, Tab::Chat);
    o.screen.press(KeyCode::Tab);
    o.screen.press(KeyCode::Tab);
    assert_eq!(o.screen.tui.app.project_ui().unwrap().tab, Tab::Team);
    o.screen.press(KeyCode::Char('5'));
    assert_eq!(o.screen.tui.app.project_ui().unwrap().tab, Tab::Tasks);
    // A task raised from elsewhere puts a dot on Tasks while Team is open.
    o.screen.press(KeyCode::Char('3'));
    let client = o.run.client.clone();
    o.run
        .rt
        .block_on(client.raise_task(o.project, "Dot me", "Raised elsewhere", None))
        .unwrap();
    let text = o.wait("Tasks has a dot", |s| s.contains("5 Tasks•"));
    shows(&text, "5 Tasks•");
    let _ = AgentState::Idle;
}
