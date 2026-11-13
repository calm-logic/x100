//! A project before and after its CEO sets it up: a new project has a CEO and nothing else, its
//! code directory is recorded once, it is set up once, and renamed only after that.

use std::path::Path;

use x100_core::action::AskMeAbout;
use x100_core::effort::EffortRange;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::fold::{Corrupt, ProjectPart, ProjectState};
use x100_core::id::{AgentId, MessageId};
use x100_core::message::{Message, Route};
use x100_core::money::{Money, Percent};
use x100_core::party::Actor;
use x100_core::project::{Identity, Settings};
use x100_core::time::Timestamp;

const CEO: AgentId = AgentId::from_u128(1);

fn created() -> ProjectEvent {
    ProjectEvent::Created {
        ceo: CEO,
        charter: "Own the outcome.".into(),
        effort: EffortRange::FULL,
        settings: Settings {
            ask_me_about: AskMeAbout::HighRiskOnly,
            budget: Money::from_dollars(50),
            lab_share: Percent::new(10).unwrap(),
            trial_before_commit: false,
            allowed: Default::default(),
        },
    }
}

fn identity(name: &str) -> Identity {
    Identity {
        name: name.into(),
        intent: "A waitlist page for a plant shop.".into(),
    }
}

/// Folds `events` one after another, returning the state and what the last one changed.
fn fold(events: Vec<ProjectEvent>) -> Result<(ProjectState, Vec<ProjectPart>), Corrupt> {
    let mut state = ProjectState::default();
    let mut parts = Vec::new();
    for (i, event) in events.into_iter().enumerate() {
        let envelope = Envelope {
            seq: Seq(i as u64 + 1),
            at: Timestamp::from_millis(1_000 + i as u64),
            by: Actor::Builder,
            event,
        };
        parts = state
            .apply(&envelope)
            .map_err(|e| e.cause)?
            .parts
            .into_iter()
            .collect();
    }
    Ok((state, parts))
}

#[test]
fn a_new_project_has_its_ceo_and_no_name_intent_or_code_directory() {
    let message = MessageId::from_u128(2);
    let (state, _) = fold(vec![
        created(),
        ProjectEvent::MessageSent(Message {
            id: message,
            route: Route::BuilderToCeo,
            body: "hi".into(),
            attachments: vec![],
            in_reply_to: None,
        }),
    ])
    .unwrap();
    let project = state.project().unwrap();
    assert_eq!(project.name(), None);
    assert!(!project.is_set_up());
    assert_eq!(project.identity, None);
    assert_eq!(project.code_dir, None);
    assert_eq!(project.ceo, CEO);
    assert_eq!(state.agent(CEO).unwrap().role, "CEO");
    assert_eq!(state.tasks().count(), 0);
    assert_eq!(state.chat().len(), 1);
}

#[test]
fn setting_up_names_the_project_and_renaming_changes_only_the_name_and_intent() {
    let dir = "/tmp/projects/fernleaf";
    let (state, parts) = fold(vec![
        created(),
        ProjectEvent::CodeDirSet { dir: dir.into() },
        ProjectEvent::SetUp(identity("Fernleaf")),
    ])
    .unwrap();
    let project = state.project().unwrap();
    assert_eq!(project.name(), Some("Fernleaf"));
    assert_eq!(
        project.identity.as_ref().unwrap().intent,
        "A waitlist page for a plant shop."
    );
    assert_eq!(project.code_dir.as_deref(), Some(Path::new(dir)));
    for part in [
        ProjectPart::Sidebar,
        ProjectPart::Chat,
        ProjectPart::Settings,
    ] {
        assert!(parts.contains(&part), "setting up changes {part:?}");
    }

    let (renamed, parts) = fold(vec![
        created(),
        ProjectEvent::CodeDirSet { dir: dir.into() },
        ProjectEvent::SetUp(identity("Fernleaf")),
        ProjectEvent::Renamed(identity("Fernleaf Shop")),
    ])
    .unwrap();
    let project = renamed.project().unwrap();
    assert_eq!(project.name(), Some("Fernleaf Shop"));
    assert_eq!(project.code_dir.as_deref(), Some(Path::new(dir)));
    assert!(parts.contains(&ProjectPart::Sidebar));
}

#[test]
fn a_project_on_an_existing_folder_has_its_directory_before_it_is_set_up() {
    let (state, _) = fold(vec![
        created(),
        ProjectEvent::CodeDirSet {
            dir: "/home/b/shop".into(),
        },
    ])
    .unwrap();
    let project = state.project().unwrap();
    assert!(!project.is_set_up());
    assert_eq!(project.code_dir.as_deref(), Some(Path::new("/home/b/shop")));
}

#[test]
fn out_of_turn_naming_and_a_moved_directory_are_errors() {
    let renamed_first = fold(vec![created(), ProjectEvent::Renamed(identity("A"))]);
    assert_eq!(renamed_first.unwrap_err(), Corrupt::NotSetUp);

    let twice = fold(vec![
        created(),
        ProjectEvent::SetUp(identity("A")),
        ProjectEvent::SetUp(identity("B")),
    ]);
    assert_eq!(twice.unwrap_err(), Corrupt::AlreadySetUp);

    let moved = fold(vec![
        created(),
        ProjectEvent::CodeDirSet { dir: "/a".into() },
        ProjectEvent::CodeDirSet { dir: "/b".into() },
    ]);
    assert_eq!(moved.unwrap_err(), Corrupt::CodeDirAlreadySet);
}
