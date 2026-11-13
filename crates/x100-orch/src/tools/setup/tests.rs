//! `set_up_project` decided from the folded state: who may call it, what the first call opens, and
//! that a later one only renames.

use std::path::PathBuf;

use x100_core::action::AskMeAbout;
use x100_core::effort::EffortRange;
use x100_core::event::{Envelope, ProjectEvent, Seq};
use x100_core::file::{ContentHash, FileRef};
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, FileId, MessageId};
use x100_core::inbox::InboxItem;
use x100_core::message::{Message, Route};
use x100_core::money::{Money, Percent};
use x100_core::party::{Actor, Party};
use x100_core::project::Settings;
use x100_core::settings::DaemonSettings;
use x100_core::task::TaskState;
use x100_core::testing::fernleaf;
use x100_core::time::{Clock, FakeClock, Timestamp};
use x100_core::tool::ToolCall;

use super::slug;
use crate::tools::{Ctx, NewCodeDir, Plan, plan};

const CEO: AgentId = AgentId::from_u128(1);
const SKETCH: FileId = FileId::from_u128(5);

/// A new project: created, and the builder has said hello and then sent a sketch.
fn new_project() -> ProjectState {
    let created = ProjectEvent::Created {
        ceo: CEO,
        charter: "Own it.".into(),
        effort: EffortRange::FULL,
        settings: Settings {
            ask_me_about: AskMeAbout::HighRiskOnly,
            budget: Money::from_dollars(50),
            lab_share: Percent::new(10).unwrap(),
            trial_before_commit: false,
            allowed: Default::default(),
        },
    };
    let message = |id: u128, body: &str, attachments: Vec<FileId>| {
        ProjectEvent::MessageSent(Message {
            id: MessageId::from_u128(id),
            route: Route::BuilderToCeo,
            body: body.into(),
            attachments,
            in_reply_to: None,
        })
    };
    let file = ProjectEvent::FileAdded(FileRef {
        id: SKETCH,
        name: "sketch.png".into(),
        media_type: "image/png".into(),
        size: 10,
        hash: ContentHash("sha256:x".into()),
        added_by: Party::Builder,
        attached_to: Some(MessageId::from_u128(3)),
    });
    let events = [
        created,
        message(2, "hi", vec![]),
        file,
        message(3, "A todo app, like this sketch.", vec![SKETCH]),
    ];
    let mut state = ProjectState::default();
    for (i, event) in events.into_iter().enumerate() {
        state
            .apply(&Envelope {
                seq: Seq(i as u64 + 1),
                at: Timestamp::from_millis(1_000),
                by: Actor::Builder,
                event,
            })
            .unwrap();
    }
    state
}

fn call(
    state: &ProjectState,
    agent: AgentId,
    root: Option<&str>,
    c: &ToolCall,
) -> Result<Plan, String> {
    let clock = FakeClock::new(Timestamp::from_millis(2_000));
    let settings = DaemonSettings {
        projects_root: root.map(PathBuf::from),
        ..DaemonSettings::default()
    };
    let ctx = Ctx {
        agent,
        now: clock.now(),
        settings: &settings,
        clock: &clock,
        dice: &x100_core::dice::SeededDice::new(1),
        file_stored: false,
        homebox: None,
    };
    plan(state, &ctx, c)
}

fn set_up(name: &str, brief: Option<&str>) -> ToolCall {
    ToolCall::SetUpProject {
        name: name.into(),
        intent: "A todo list in one page.".into(),
        brief: brief.map(Into::into),
    }
}

fn fold(state: &ProjectState, plan: Plan) -> ProjectState {
    let mut state = state.clone();
    for (by, event) in plan.events {
        let seq = Seq(state.seq().unwrap().0 + 1);
        let at = Timestamp::from_millis(2_000);
        state.apply(&Envelope { seq, at, by, event }).unwrap();
    }
    state
}

#[test]
fn the_first_call_sets_the_project_up_and_opens_the_builders_task() {
    let state = new_project();
    let brief = "One index.html with a todo list that survives a reload.";
    let plan = call(&state, CEO, Some("/x"), &set_up("Todo app", Some(brief))).unwrap();
    assert_eq!(
        plan.code_dir,
        Some(NewCodeDir {
            root: "/x".into(),
            slug: "todo-app".into()
        })
    );
    let after = fold(&state, plan);
    let project = after.project().unwrap();
    assert_eq!(project.name(), Some("Todo app"));
    let task = after.tasks().next().expect("a task");
    assert_eq!(task.task.ask, brief);
    assert_eq!(task.task.raised_by, Party::Builder);
    assert_eq!(task.task.attachments, vec![SKETCH]);
    let TaskState::InProgress(a) = &task.state else {
        panic!("not in progress")
    };
    assert_eq!((a.delegator, a.assignee), (Party::Builder, CEO));
    let ceo = after.agent(CEO).unwrap();
    let delivered: Vec<_> = ceo.inbox.waiting().map(|i| ceo.items[&i].clone()).collect();
    assert!(delivered.contains(&InboxItem::Task { task: task.task.id }));
}

#[test]
fn a_project_on_an_existing_folder_gets_no_new_directory() {
    let mut state = new_project();
    let seq = Seq(state.seq().unwrap().0 + 1);
    let event = ProjectEvent::CodeDirSet {
        dir: "/home/b/shop".into(),
    };
    let at = Timestamp::from_millis(1_500);
    state
        .apply(&Envelope {
            seq,
            at,
            by: Actor::Builder,
            event,
        })
        .unwrap();
    let plan = call(&state, CEO, Some("/x"), &set_up("Shop", Some("Fix it."))).unwrap();
    assert_eq!(plan.code_dir, None);
}

#[test]
fn the_first_call_needs_a_name_an_intent_a_brief_and_a_projects_root() {
    let state = new_project();
    let empty = call(&state, CEO, Some("/x"), &set_up("  ", Some("b"))).unwrap_err();
    assert!(empty.contains("`name` is empty"), "{empty}");
    let no_intent = ToolCall::SetUpProject {
        name: "Todo".into(),
        intent: "".into(),
        brief: Some("b".into()),
    };
    let e = call(&state, CEO, Some("/x"), &no_intent).unwrap_err();
    assert!(e.contains("`intent` is empty"), "{e}");
    let e = call(&state, CEO, Some("/x"), &set_up("Todo", None)).unwrap_err();
    assert!(e.contains("needs `brief`"), "{e}");
    let e = call(&state, CEO, None, &set_up("Todo", Some("b"))).unwrap_err();
    assert!(e.contains("no projects root"), "{e}");
}

#[test]
fn only_the_ceo_sets_a_project_up() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let e = call(&state, f.tech, Some("/x"), &set_up("Mine", Some("b"))).unwrap_err();
    assert_eq!(e, "only the CEO calls `set_up_project`");
}

#[test]
fn a_later_call_renames_and_does_nothing_else() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let plan = call(
        &state,
        f.ceo,
        Some("/x"),
        &set_up("Fernleaf Shop", Some("b")),
    )
    .unwrap();
    assert_eq!(plan.code_dir, None);
    assert!(plan.commit.is_empty() && plan.merge.is_none());
    assert!(
        matches!(plan.events.as_slice(), [(_, ProjectEvent::Renamed(i))] if i.name == "Fernleaf Shop")
    );
    let tasks = state.tasks().count();
    let after = fold(&state, plan);
    assert_eq!(after.tasks().count(), tasks);
    assert_eq!(
        after.project().unwrap().code_dir,
        state.project().unwrap().code_dir
    );

    let same = ToolCall::SetUpProject {
        name: "Fernleaf".into(),
        intent: "A waitlist page for Fernleaf, a plant shop.".into(),
        brief: None,
    };
    let e = call(&state, f.ceo, Some("/x"), &same).unwrap_err();
    assert!(e.contains("already has this name"), "{e}");
}

#[test]
fn a_slug_is_lower_case_words_joined_by_dashes() {
    assert_eq!(slug("Todo app"), "todo-app");
    assert_eq!(slug("  Fernleaf: the Shop!! "), "fernleaf-the-shop");
    assert_eq!(slug("Café 2"), "caf-2");
    assert_eq!(slug("???"), "project");
}
