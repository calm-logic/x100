//! Held actions in the app against a real daemon on the fake clock: at "Every decision" the CEO's
//! first calls wait in the strip above the composer, and the builder's Approve and Deny, clicked
//! there, reach the daemon. The approved call is carried out (its message reaches the chat), the
//! denied one is not, and each leaves the strip.

mod common;

use common::*;
use gpui::VisualTestContext;
use x100_core::action::{ActionState, AskMeAbout, Request};
use x100_core::fold::ProjectState;
use x100_core::message::Route;
use x100_core::project::SettingChange;
use x100_core::tool::ToolCall;
use x100_proto::ChangeProjectSetting;

fn held_call(s: &ProjectState, f: impl Fn(&ToolCall) -> bool) -> Option<x100_core::id::ActionId> {
    s.held_actions()
        .find(|a| matches!(&a.action.request, Request::Tool { call, .. } if f(call)))
        .map(|a| a.action.id)
}

#[test]
fn approve_and_deny_from_the_strip_reach_the_daemon() {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
    run.rt
        .block_on(client.request(ChangeProjectSetting {
            project,
            change: SettingChange::AskMeAbout(AskMeAbout::EveryDecision),
        }))
        .expect("the level is set");
    run.rt
        .block_on(client.send_chat(
            project,
            "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
            vec![],
            None,
        ))
        .expect("the first message is sent");
    // The CEO's answer and its setting the project up both wait for the builder.
    let (message, set_up) = run
        .rt
        .block_on(client.wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            let message = held_call(p, |c| matches!(c, ToolCall::Message { .. }))?;
            let set_up = held_call(p, |c| matches!(c, ToolCall::SetUpProject { .. }))?;
            Some((message, set_up))
        }))
        .expect("two calls are held");
    let chat_from_ceo =
        |s: &ProjectState| s.messages().any(|m| m.message.route == Route::CeoToBuilder);
    assert!(!chat_from_ceo(&client.project(project).unwrap()));

    let (window, mut vcx) = run.open(client.clone());
    let held = vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .chat
            .read(cx)
            .held()
            .clone()
    });
    pump_until(&mut vcx, "both are in the strip", |vcx| {
        let shown: Vec<_> = vcx.read(|cx| held.read(cx).shown().iter().map(|h| h.id).collect());
        shown.contains(&message) && shown.contains(&set_up)
    });
    let at = |vcx: &mut VisualTestContext, id| {
        vcx.read(|cx| held.read(cx).shown().iter().position(|h| h.id == id))
            .expect("in the strip")
    };

    // Approve: the daemon carries the held call out, and the CEO's message reaches the chat.
    let i = at(&mut vcx, message);
    click(&mut vcx, leak(format!("approve-{i}")));
    let state = |client: &x100_client::Client| client.project(project).unwrap();
    let c = client.clone();
    pump_until(&mut vcx, "the approved call is carried out", move |_| {
        let s = state(&c);
        s.action(message).unwrap().state == ActionState::CarriedOut && chat_from_ceo(&s)
    });
    pump_until(&mut vcx, "it leaves the strip", |vcx| {
        vcx.read(|cx| held.read(cx).shown().iter().all(|h| h.id != message))
    });

    // Deny: nothing is done, and it leaves the strip too.
    let i = at(&mut vcx, set_up);
    click(&mut vcx, leak(format!("deny-{i}")));
    let c = client.clone();
    pump_until(&mut vcx, "the denied call is dropped", move |_| {
        state(&c).action(set_up).unwrap().state == ActionState::Denied
    });
    pump_until(&mut vcx, "it leaves the strip", |vcx| {
        vcx.read(|cx| held.read(cx).shown().iter().all(|h| h.id != set_up))
    });
    let s = state(&client);
    assert!(
        s.project().unwrap().identity.is_none(),
        "the project was not set up"
    );
    assert_eq!(
        vcx.read(|cx| held.read(cx).outcome(set_up).cloned()),
        None,
        "no error was shown"
    );
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}
