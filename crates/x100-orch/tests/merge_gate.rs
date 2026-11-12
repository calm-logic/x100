//! A merge into main the gate holds, in-process on the fake clock. At "Medium and up" the CEO's
//! report on the builder's task leaves the task Promoting, with the merge held. Approve merges it
//! and Deny sends the task back to the CEO; either way the CEO is told through its inbox.

mod common;

use common::*;
use x100_core::action::{ActionState, AskMeAbout, Decision, Request};
use x100_core::event::ProjectEvent;
use x100_core::inbox::InboxItem;
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_core::task::TaskState;

const SCRIPT: &str = r#"
[harness]
name = "fake-merges"

[[model]]
model = "fake-long"
context_window = 200000
cache_lifetime_secs = 604800
efforts = ["low", "medium", "high", "max"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "Ship it" }
repeat = true
steps = [
  { write = "site.html", content = "<p>Fernleaf</p>" },
  { call = { report = { task = "{{input:Current task}}", outcome = "completed", summary = "Shipped." } } },
]

[[turn]]
repeat = true
steps = [{ text = "noted: {{input}}" }]
"#;

async fn held_merge(w: &World) -> (x100_core::id::ActionId, x100_core::id::TaskId) {
    w.store
        .log
        .append(
            Actor::Builder,
            ProjectEvent::SettingChanged(SettingChange::AskMeAbout(AskMeAbout::MediumAndUp)),
        )
        .unwrap();
    let task = w.send("Ship it").await;
    w.until("the merge is held", |s| {
        s.held_actions()
            .any(|a| matches!(a.action.request, Request::Merge { .. }))
    })
    .await;
    let s = w.state();
    let merge = s
        .held_actions()
        .find(|a| a.action.request == Request::Merge { task })
        .expect("the task's merge")
        .action
        .id;
    assert!(matches!(
        s.task(task).unwrap().state,
        TaskState::Promoting(..)
    ));
    (merge, task)
}

fn told(w: &World, action: x100_core::id::ActionId) -> Option<Option<String>> {
    w.log().into_iter().find_map(|e| match e.event {
        ProjectEvent::Agent {
            event:
                x100_core::agent::AgentEvent::Delivered {
                    what:
                        InboxItem::Decision {
                            action: a, result, ..
                        },
                    ..
                },
            ..
        } if a == action => Some(result),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approved_merge_into_main_is_done_and_the_ceo_told() {
    let w = World::new(Setup::new(SCRIPT)).await;
    let (merge, task) = held_merge(&w).await;
    assert!(
        read(&w.project_dir().join("site.html")).is_none(),
        "main is untouched"
    );

    w.orch
        .decide(w.project, merge, Decision::Approved)
        .await
        .unwrap();
    let s = w.state();
    assert_eq!(s.action(merge).unwrap().state, ActionState::CarriedOut);
    assert!(matches!(s.task(task).unwrap().state, TaskState::Done(..)));
    assert_eq!(
        read(&w.project_dir().join("site.html")).as_deref(),
        Some("<p>Fernleaf</p>")
    );
    assert_eq!(told(&w, merge), Some(Some("The merge is done.".into())));
    // Decided once, it cannot be decided again.
    assert!(
        w.orch
            .decide(w.project, merge, Decision::Denied)
            .await
            .is_err()
    );
    w.settle().await;
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_merge_sends_the_task_back_and_the_ceo_is_told() {
    let w = World::new(Setup::new(SCRIPT)).await;
    let (merge, task) = held_merge(&w).await;
    w.orch
        .decide(w.project, merge, Decision::Denied)
        .await
        .unwrap();
    let s = w.state();
    assert_eq!(s.action(merge).unwrap().state, ActionState::Denied);
    assert!(
        matches!(s.task(task).unwrap().state, TaskState::InProgress(_)),
        "{:?}",
        s.task(task).unwrap().state
    );
    assert!(
        read(&w.project_dir().join("site.html")).is_none(),
        "main is untouched"
    );
    let result = told(&w, merge).expect("the CEO is told").unwrap();
    assert!(result.contains("went back"), "{result}");
    w.finish().await;
}
