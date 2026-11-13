//! Held actions against a real daemon on the fake clock: at "Every decision" the CEO's first calls
//! wait on the Held tab, and Approve and Deny, pressed there, reach the daemon. The approved call
//! is carried out, the denied one is not, and each leaves the tab.

mod common;

use common::*;
use ratatui::crossterm::event::KeyCode;
use x100_client::wording::{self, Names};
use x100_core::action::{ActionState, AskMeAbout, Request};
use x100_core::fold::ProjectState;
use x100_core::id::ActionId;
use x100_core::message::Route;
use x100_core::project::SettingChange;
use x100_core::tool::ToolCall;
use x100_proto::ChangeProjectSetting;
use x100_tui::app::Tab;
use x100_tui::page::Target;
use x100_tui::remember::Remember;

fn held_call(s: &ProjectState, f: impl Fn(&ToolCall) -> bool) -> Option<ActionId> {
    s.held_actions()
        .find(|a| matches!(&a.action.request, Request::Tool { call, .. } if f(call)))
        .map(|a| a.action.id)
}

#[test]
fn approve_and_deny_on_the_held_tab_reach_the_daemon() {
    let run = Run::start();
    let project = run.fernleaf_project();
    run.rt
        .block_on(run.client.request(ChangeProjectSetting {
            project,
            change: SettingChange::AskMeAbout(AskMeAbout::EveryDecision),
        }))
        .expect("the level is set");
    let mut screen = run.tui(Remember::none());
    screen.tui.app.open_project(project);
    screen.tui.app.set_tab(Tab::Held);
    run.rt
        .block_on(run.client.send_chat(project, BRIEF, vec![], None))
        .expect("the first message is sent");
    // The CEO's answer and its setting the project up both wait for the builder.
    let (message, set_up) = run
        .rt
        .block_on(run.client.wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            let message = held_call(p, |c| matches!(c, ToolCall::Message { .. }))?;
            let set_up = held_call(p, |c| matches!(c, ToolCall::SetUpProject { .. }))?;
            Some((message, set_up))
        }))
        .expect("two calls are held");
    let state = || run.client.project(project).unwrap();
    let chat_from_ceo =
        |s: &ProjectState| s.messages().any(|m| m.message.route == Route::CeoToBuilder);
    assert!(!chat_from_ceo(&state()));

    // Both are listed with who wants what, as the Log words them.
    let text = screen.wait_for(&run.rt, "both are listed", |s| {
        s.matches(" wants to ").count() == 2
    });
    let daemon = run.client.state().daemon;
    let p = state();
    let files = Default::default();
    let names = Names {
        project: &p,
        daemon: &daemon,
        files: &files,
    };
    for id in [message, set_up] {
        let a = p.action(id).unwrap();
        let what = wording::request(&names, &a.action.request);
        assert!(
            text.contains(&format!("CEO wants to {what}")),
            "{what} is not on\n{text}"
        );
    }

    // Approve: the daemon carries the held call out, and the CEO's message reaches the chat.
    let select = |screen: &mut Screen, id: ActionId| {
        for _ in 0..4 {
            if screen.tui.app.selected() == Some(Target::Action(id)) {
                return;
            }
            screen.press(KeyCode::Down);
        }
        panic!("{id} was not selectable");
    };
    select(&mut screen, message);
    screen.press(KeyCode::Char('a'));
    screen.wait_for(&run.rt, "the approved call is carried out", |_| {
        let s = state();
        s.action(message).unwrap().state == ActionState::CarriedOut && chat_from_ceo(&s)
    });
    let text = screen.wait_for(&run.rt, "it leaves the tab", |s| {
        s.matches(" wants to ").count() == 1
    });
    assert!(text.contains("set the project up as"), "{text}");

    // Deny: nothing is done, and it leaves the tab too.
    select(&mut screen, set_up);
    screen.press(KeyCode::Char('d'));
    screen.wait_for(&run.rt, "the denied call is dropped", |_| {
        state().action(set_up).unwrap().state == ActionState::Denied
    });
    let text = screen.wait_for(&run.rt, "nothing waits", |s| {
        s.contains("Nothing waits for you.")
    });
    assert!(!text.contains(" wants to "));
    assert!(
        state().project().unwrap().identity.is_none(),
        "the project was not set up"
    );
}
