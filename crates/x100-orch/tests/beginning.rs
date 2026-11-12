//! How a project begins: the builder's first message reaches a CEO whose project has no name and
//! no code, and the CEO sets the project up once it knows what it is. In-process, on the fake
//! harness and the fake clock.

mod common;

use std::path::Path;

use common::*;
use x100_core::agent::AgentState;
use x100_core::driver::{DriverState, EndReason, RetireReason, StartReason};
use x100_core::event::ProjectEvent;
use x100_core::file::{ContentHash, FileRef};
use x100_core::fold::ProjectState;
use x100_core::id::FileId;
use x100_core::inbox::InboxItem;
use x100_core::message::Route;
use x100_core::party::{Actor, Party};
use x100_core::task::TaskState;

const SCRIPT: &str = r#"
[harness]
name = "fake-beginning"

[[model]]
model = "fake-one"
context_window = 200000
cache_lifetime_secs = 604800
efforts = ["low", "medium", "high", "max"]
price = { input = 1.0, output = 5.0 }

# A greeting tells the CEO nothing about the project, so it only answers.
[[turn]]
when = { start_pack_contains = "PROJECT: not set up yet", input_contains = "hi there" }
steps = [{ text = "Hello! What would you like to build?" }]

[[turn]]
when = { start_pack_contains = "PROJECT: not set up yet", input_contains = "todo app" }
steps = [
  { text = "A todo app: setting it up." },
  { call = { set_up_project = { name = "Todo app", intent = "A todo list in one page.", brief = "Build a todo app\nOne index.html with a list that survives a reload." } } },
]

[[turn]]
when = { start_pack_contains = "PROJECT: Todo app", input_contains = "A task for you" }
steps = [
  { write = "index.html", content = "<ul id=todos></ul>\n" },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "index.html is on main." } } },
]

[[turn]]
when = { input_contains = "call it Tasks" }
steps = [
  { call = { set_up_project = { name = "Tasks", intent = "A todo list in one page." } } },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "Renamed." } } },
]
"#;

const BRIEF: &str = "Build me a todo app, one page, like the sketch.";

async fn world(start: Start) -> World {
    World::new(Setup {
        start,
        ..Setup::new(SCRIPT)
    })
    .await
}

/// A file the builder uploaded, as the daemon's upload records it.
fn upload(w: &World) -> FileId {
    let file = FileId::from_u128(77);
    let added = ProjectEvent::FileAdded(FileRef {
        id: file,
        name: "sketch.png".into(),
        media_type: "image/png".into(),
        size: 3,
        hash: ContentHash("sha256:sketch".into()),
        added_by: Party::Builder,
        attached_to: None,
    });
    w.store.log.append(Actor::Builder, added).unwrap();
    file
}

fn ceo_drivers(s: &ProjectState) -> Vec<x100_core::fold::DriverRecord> {
    let ceo = s.project().unwrap().ceo;
    let a = s.agent(ceo).unwrap();
    a.drivers
        .iter()
        .map(|d| s.driver(*d).unwrap().clone())
        .collect()
}

fn nudged(w: &World) -> bool {
    w.log().iter().any(|e| {
        matches!(
            &e.event,
            ProjectEvent::Agent {
                event: x100_core::agent::AgentEvent::Delivered {
                    what: InboxItem::Nudge { .. },
                    ..
                },
                ..
            }
        )
    })
}

fn replies(s: &ProjectState) -> Vec<String> {
    s.messages()
        .filter(|m| m.message.route == Route::CeoToBuilder)
        .map(|m| m.message.body.clone())
        .collect()
}

/// Everything after the builder's brief: the project is Todo app in `<root>/todo-app`, the task
/// carries the CEO's wording and the sketch, the scratch driver retired, and a new driver in the
/// worktree did the work, which is on main.
fn assert_set_up_and_done(w: &World, sketch: FileId) {
    let s = w.state();
    let project = s.project().unwrap();
    assert_eq!(project.name(), Some("Todo app"));
    let code = w.projects_root().join("todo-app");
    assert_eq!(project.code_dir.as_deref(), Some(code.as_path()));
    assert!(
        code.join(".git").exists(),
        "the code directory has a repository"
    );

    let task = s.tasks().next().expect("the builder's task");
    assert_eq!(s.tasks().count(), 1);
    assert_eq!(
        task.task.ask,
        "Build a todo app\nOne index.html with a list that survives a reload."
    );
    assert_eq!(task.task.title, "Build a todo app");
    assert_eq!(task.task.attachments, vec![sketch]);
    assert!(
        matches!(task.state, TaskState::Done(..)),
        "{:?}",
        task.state
    );

    let drivers = ceo_drivers(&s);
    assert_eq!(
        drivers.len(),
        2,
        "the scratch driver and the one in the worktree"
    );
    assert_eq!(
        drivers[0].state,
        DriverState::Ended(EndReason::Retired(RetireReason::ProjectSetUp))
    );
    assert_eq!(
        drivers[1].driver.why,
        StartReason::NewTask { task: task.task.id }
    );
    assert_eq!(
        std::fs::read_to_string(code.join("index.html")).unwrap(),
        "<ul id=todos></ul>\n",
        "the work is on main"
    );
    assert!(!nudged(w), "no turn was nudged");
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_gets_an_answer_and_no_task_then_a_brief_sets_the_project_up() {
    let w = world(Start::New).await;
    w.say("hi there").await;
    w.settle().await;

    let s = w.state();
    let ceo = s.project().unwrap().ceo;
    assert!(!s.project().unwrap().is_set_up());
    assert_eq!(
        s.tasks().count(),
        0,
        "a message to a new project opens no task"
    );
    assert_eq!(replies(&s), vec!["Hello! What would you like to build?"]);
    assert_eq!(s.agent(ceo).unwrap().state, AgentState::Idle);
    assert!(!nudged(&w), "a CEO with no task is not nudged");
    assert!(
        w.store.scratch().is_dir(),
        "the driver ran in the scratch directory"
    );
    assert!(!w.projects_root().exists(), "no code directory yet");

    let sketch = upload(&w);
    w.say_with(BRIEF, vec![sketch]).await;
    w.settle().await;
    assert_set_up_and_done(&w, sketch);

    // The new driver starts from a pack with the conversation so far, its task and the project.
    let s = w.state();
    let second = ceo_drivers(&s)[1].driver.id;
    let pack = w
        .orch
        .start_pack(w.project, second)
        .expect("its start pack");
    for needed in [
        "PROJECT: Todo app\nA todo list in one page.",
        "- The builder: hi there",
        "- You: Hello! What would you like to build?",
        &format!("- The builder: {BRIEF} [1 attached]"),
        "- You: A todo app: setting it up.",
        "CURRENT TASK:",
        "One index.html with a list that survives a reload.",
        "sketch.png",
    ] {
        assert!(pack.contains(needed), "the pack lacks {needed:?}:\n{pack}");
    }
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clear_brief_as_the_first_message_sets_the_project_up_at_once() {
    let w = world(Start::New).await;
    let sketch = upload(&w);
    w.say_with(BRIEF, vec![sketch]).await;
    w.settle().await;
    assert_set_up_and_done(&w, sketch);
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_later_call_renames_the_project_and_moves_nothing() {
    let w = world(Start::New).await;
    let sketch = upload(&w);
    w.say_with(BRIEF, vec![sketch]).await;
    w.settle().await;
    let before = w.project_dir();

    w.say("Please call it Tasks.").await;
    w.settle().await;
    let s = w.state();
    let project = s.project().unwrap();
    assert_eq!(project.name(), Some("Tasks"));
    assert_eq!(project.code_dir.as_deref(), Some(before.as_path()));
    assert!(before.join("index.html").exists());
    let dirs: Vec<_> = std::fs::read_dir(w.projects_root())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        dirs,
        vec!["todo-app"],
        "nothing new under the projects root"
    );
    assert!(
        w.log()
            .iter()
            .any(|e| matches!(&e.event, ProjectEvent::Renamed(i) if i.name == "Tasks"))
    );
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_projects_given_the_same_name_get_different_directories() {
    let root = tempfile::tempdir().unwrap();
    let mut worlds = Vec::new();
    for _ in 0..2 {
        let mut setup = Setup::new(SCRIPT);
        setup.start = Start::New;
        setup.settings.projects_root = Some(root.path().to_owned());
        let w = World::new(setup).await;
        w.say(BRIEF).await;
        w.settle().await;
        worlds.push(w);
    }
    let dirs: Vec<_> = worlds.iter().map(World::project_dir).collect();
    assert_eq!(dirs[0], root.path().join("todo-app"));
    assert_eq!(dirs[1], root.path().join("todo-app-2"));
    for w in worlds {
        w.finish().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_existing_folder_gives_the_ceo_a_worktree_there_at_once() {
    let w = world(Start::Existing).await;
    let existing = w.dir.path().join("existing");
    w.say("hi there").await;
    w.settle().await;

    let s = w.state();
    let ceo = s.project().unwrap().ceo;
    assert!(!s.project().unwrap().is_set_up());
    let worktree = existing.join(".x100/worktrees").join(ceo.to_string());
    assert!(
        worktree.join("README.md").exists(),
        "the CEO's worktree has the code to read"
    );
    assert!(!w.store.scratch().exists(), "no scratch directory");

    w.say(BRIEF).await;
    w.settle().await;
    let s = w.state();
    let project = s.project().unwrap();
    assert_eq!(project.name(), Some("Todo app"));
    assert_eq!(project.code_dir.as_deref(), Some(existing.as_path()));
    assert!(
        !w.projects_root().exists(),
        "an existing folder needs no new directory"
    );
    // The router may give the task a driver at a higher effort, but none is retired for moving.
    let moved = ceo_drivers(&s)
        .into_iter()
        .any(|d| d.state == DriverState::Ended(EndReason::Retired(RetireReason::ProjectSetUp)));
    assert!(!moved, "a driver in the worktree has nowhere to move");
    assert!(matches!(
        s.tasks().next().unwrap().state,
        TaskState::Done(..)
    ));
    assert!(Path::new(&existing.join("index.html")).exists());
    assert!(!nudged(&w));
    w.finish().await;
}
