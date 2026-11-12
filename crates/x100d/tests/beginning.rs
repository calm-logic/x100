//! A project's beginning through the daemon as a child process, with the client library as the
//! only way in: the builder's first message creates the project, a client already following sees
//! it, and a restart before the CEO sets it up loses neither the project nor the conversation.

mod common;

use common::*;
use x100_core::agent::AgentState;
use x100_core::fold::ProjectState;
use x100_core::message::Route;
use x100_proto::ListProjects;

/// The builder's messages and the CEO's answers, in order.
fn conversation(p: &ProjectState) -> Vec<(Route, String)> {
    p.messages()
        .map(|m| (m.message.route, m.message.body.clone()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_message_creates_a_project_that_survives_a_restart_before_it_is_set_up() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    assert!(client.with_state(|s| s.projects.is_empty()));

    // As the app does it: the project is created as the first message is sent.
    let project = client.create_project(None).await.unwrap();
    client
        .send_chat(project, "hello there", vec![], None)
        .await
        .unwrap();
    // The client was following before the project existed, and its events reached it.
    let s = client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            let ceo = p.project()?.ceo;
            let answered = p.messages().any(|m| m.message.route == Route::CeoToBuilder);
            (answered && p.agent(ceo)?.state == AgentState::Idle).then(|| p.clone())
        })
        .await
        .expect("the follower sees the project and the CEO's answer");
    let p = s.project().unwrap();
    assert_eq!((p.name(), p.code_dir.as_ref()), (None, None));
    assert_eq!(s.tasks().count(), 0);
    let before = conversation(&s);
    assert_eq!(before.len(), 2);
    assert_eq!(before[0], (Route::BuilderToCeo, "hello there".to_owned()));
    let listed = client.request(ListProjects {}).await.unwrap().projects;
    assert_eq!(
        (
            listed.len(),
            listed[0].name.as_ref(),
            listed[0].dir.as_ref()
        ),
        (1, None, None)
    );

    // Its state is in the daemon's data directory under its id, and nothing is in the root.
    assert!(
        x100_store::project_dir(&env.data, project)
            .join("store.sqlite")
            .exists()
    );
    assert!(!env.projects_root().exists());

    daemon.terminate();
    drop(client);
    let mut daemon = env.spawn();
    let client = env.client().await;
    let s = client
        .wait_for(WAIT, |s| s.projects.get(&project).cloned())
        .await
        .expect("the project is back");
    assert_eq!(conversation(&s), before, "the conversation is kept");
    assert!(!s.project().unwrap().is_set_up());

    // And it carries on: a further message is answered.
    client
        .send_chat(project, "still there?", vec![], None)
        .await
        .unwrap();
    client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            (p.messages()
                .filter(|m| m.message.route == Route::CeoToBuilder)
                .count()
                == 2)
                .then_some(())
        })
        .await
        .expect("the CEO answers after the restart");
    daemon.terminate();
}
