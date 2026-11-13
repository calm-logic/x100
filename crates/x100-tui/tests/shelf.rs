//! An agent's notes and skills against a real daemon with a script of this test's own, on the
//! fake clock:
//!
//! - The CEO remembers a note when the builder asks; its page shows the note's name, when it was
//!   written and what it says, as the daemon reads them, and the skills it can load with their
//!   descriptions.
//! - From the skills page the builder writes a skill of the CEO's own, its text over several
//!   lines, and the daemon has it as version 1; rewriting it from its words makes version 2. The
//!   page shows each version as the daemon then reads it.

mod common;

use common::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use x100_core::id::{AgentId, ProjectId};
use x100_core::memory::{SkillName, SkillRef, SkillScope};
use x100_proto::CreateProject;
use x100_tui::app::{AgentMode, Tab};
use x100_tui::page::Target;
use x100_tui::remember::Remember;

const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "REMEMBER" }
repeat = true
steps = [
  { call = { remember = { note = "Deploys", body = "Deploys go through `make ship`, never by hand.\n\nThe builder said so in the first message.", sources = [1] } }, save = "r" },
  { call = { message = { to = "builder", body = "remembered", attachments = [] } } },
]

[[turn]]
repeat = true
steps = [
  { call = { message = { to = "builder", body = "ok", attachments = [] } } },
]
"##;

struct Shelf {
    run: Run,
    project: ProjectId,
    ceo: AgentId,
    screen: Screen,
}

/// A project on a folder of its own whose CEO has remembered a note, and the interface open on
/// the CEO's page.
fn remembered() -> Shelf {
    let run = Run::scripted(SCRIPT, &[]);
    let dir = run.daemon.dir.path().join("shelf");
    std::fs::create_dir_all(&dir).expect("the project's folder");
    let client = run.client.clone();
    let project = run.rt.block_on(async {
        let project = client
            .request(CreateProject {
                dir: Some(dir.to_string_lossy().into_owned()),
                settings: None,
            })
            .await
            .expect("the project is created")
            .project;
        client
            .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|_| ()))
            .await
            .expect("the client holds it");
        client
            .send_chat(project, "REMEMBER how deploys go", vec![], None)
            .await
            .expect("the message is sent");
        client
            .wait_for(WAIT, |s| {
                let p = s.projects.get(&project)?;
                p.messages()
                    .any(|m| m.message.body == "remembered")
                    .then_some(())
            })
            .await
            .expect("the CEO remembers");
        project
    });
    run.settle(project);
    let ceo = client.project(project).unwrap().project().unwrap().ceo;
    let mut screen = run.tui(Remember::none());
    screen.tui.app.open_project(project);
    screen.tui.app.set_tab(Tab::Team);
    screen.tui.app.open_agent(ceo);
    Shelf {
        run,
        project,
        ceo,
        screen,
    }
}

impl Shelf {
    fn wait(&mut self, what: &str, done: impl FnMut(&str) -> bool) -> String {
        self.screen.wait_for(&self.run.rt, what, done)
    }

    fn skills(&self) -> x100_proto::Skills {
        self.run
            .rt
            .block_on(self.run.client.list_skills(self.project, None))
            .expect("the daemon reads the skills")
    }

    fn answer(&mut self, text: &str) {
        self.screen.type_text(text);
        self.screen.press(KeyCode::Enter);
    }
}

#[test]
fn an_agents_notes_and_skills_are_shown_as_the_daemon_reads_them() {
    let mut s = remembered();
    let notes = s
        .run
        .rt
        .block_on(s.run.client.list_memories(s.project, s.ceo))
        .expect("the daemon reads the notes");
    assert_eq!(notes.notes.len(), 1, "{notes:?}");
    let note = &notes.notes[0];

    s.screen.press(KeyCode::Char('m'));
    let text = s.wait("the notes are read", |t| !t.contains("Reading its notes"));
    let heading = format!(
        "{} · written {}",
        note.name.0,
        x100_client::words::moment(note.written)
    );
    assert!(text.contains(&heading), "{heading:?} is not on\n{text}");
    for line in note.body.lines().filter(|l| !l.is_empty()) {
        assert!(
            text.contains(&format!("  {line}")),
            "{line:?} is not on\n{text}"
        );
    }

    let skills = s.skills();
    assert!(!skills.skills.is_empty(), "the built-in skill is there");
    s.screen.press(KeyCode::Char('k'));
    let text = s.wait("the skills are read", |t| !t.contains("Reading its skills"));
    assert!(text.contains("SHARED SKILLS"), "{text}");
    for k in &skills.skills {
        let line = format!("{} · version {}", k.skill.name.0, k.version.0);
        assert!(text.contains(&line), "{line:?} is not on\n{text}");
        let start: String = k.description.chars().take(40).collect();
        assert!(text.contains(&start), "{start:?} is not on\n{text}");
    }
}

#[test]
fn a_skill_is_written_and_rewritten_from_the_agents_page() {
    let mut s = remembered();
    s.screen.press(KeyCode::Char('k'));
    s.wait("the skills are read", |t| !t.contains("Reading its skills"));
    assert_eq!(
        s.screen
            .tui
            .app
            .project_ui()
            .unwrap()
            .agent
            .as_ref()
            .unwrap()
            .mode,
        AgentMode::Skills
    );

    // n: its name, whose it is, when to use it, and what it says over two lines.
    s.screen.press(KeyCode::Char('n'));
    s.answer("triage");
    s.answer("a");
    s.answer("When a bug report arrives.");
    s.screen.type_text("Reproduce it first.");
    s.screen
        .tui
        .app
        .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    s.screen.type_text("Then write the failing test.");
    let typing = s.screen.text();
    assert!(
        typing.contains("> Then write the failing test."),
        "the prompt shows the line being typed:\n{typing}"
    );
    s.screen.press(KeyCode::Enter);
    let text = s.wait("the skill is written", |t| t.contains("triage · version 1"));
    assert!(text.contains("Wrote the skill triage, version 1"), "{text}");

    let skill = SkillRef {
        scope: SkillScope::Agent(s.ceo),
        name: SkillName("triage".into()),
    };
    let find = |s: &Shelf| {
        s.skills()
            .skills
            .into_iter()
            .find(|k| k.skill == skill)
            .expect("the daemon has the skill")
    };
    let written = find(&s);
    assert_eq!(written.version.0, 1);
    assert_eq!(written.description, "When a bug report arrives.");
    assert!(
        written
            .body
            .contains("\n\nReproduce it first.\nThen write the failing test.\n"),
        "{:?}",
        written.body
    );
    assert!(text.contains("When a bug report arrives."), "{text}");
    assert!(text.contains("ITS OWN SKILLS"), "{text}");

    // e on it: its words to start from; the description is changed, the text kept.
    assert!(s.screen.select(&Target::Skill(skill.clone())));
    s.screen.press(KeyCode::Char('e'));
    let asked = s.screen.text();
    assert!(
        asked.contains("> When a bug report arrives."),
        "the description is there to change:\n{asked}"
    );
    s.screen.ctrl('u');
    s.answer("When a bug is reported or found.");
    s.screen.press(KeyCode::Enter);
    let text = s.wait("the skill is rewritten", |t| {
        t.contains("triage · version 2")
    });
    let rewritten = find(&s);
    assert_eq!(rewritten.version.0, 2);
    assert_eq!(rewritten.description, "When a bug is reported or found.");
    assert!(
        rewritten
            .body
            .contains("\n\nReproduce it first.\nThen write the failing test.\n"),
        "{:?}",
        rewritten.body
    );
    assert!(text.contains("When a bug is reported or found."), "{text}");
}
