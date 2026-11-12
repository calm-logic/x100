//! Settings, Decision model, against a real daemon on the fake clock without a display: it says
//! no decision model is connected; picking OpenAI, pasting a key and connecting reaches the
//! daemon, which keeps the key in its vault scoped to OpenAI's host and to no project; the key
//! field is emptied and the key is in nothing the app holds; Disconnect removes the key.

mod common;

use std::collections::BTreeSet;

use common::*;
use gpui::{Entity, VisualTestContext};
use x100_app::settings::{DecisionPart, SettingsView};
use x100_app::tabs::Tab;
use x100_core::credential::Projects;
use x100_core::decision::DecisionProvider;

/// Invented for this test; it opens nothing anywhere.
const KEY: &str = "sk-dm-app-test-not-a-real-key-41c2";

fn part(vcx: &VisualTestContext, settings: &Entity<SettingsView>) -> Entity<DecisionPart> {
    vcx.read(|cx| settings.read(cx).decision().clone())
}

/// Scrolls the page under the pointer down, as the wheel would, to reach what is below the fold.
fn scroll_down(vcx: &mut VisualTestContext) {
    vcx.simulate_event(gpui::ScrollEvent {
        position: gpui::point(gpui::px(900.), gpui::px(500.)),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-1000.))),
        ..Default::default()
    });
    pump(vcx);
}

#[test]
fn a_decision_model_is_connected_with_a_key_and_disconnected_from_settings() {
    let mut run = Run::start();
    let client = run.client();
    let project = run.fernleaf_project(&client);
    run.rt
        .block_on(client.send_chat(
            project,
            "Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.",
            vec![],
            None,
        ))
        .expect("the first message is sent");
    run.settle(&client, project);
    let (window, mut vcx) = run.open(client.clone());
    pump_until(&mut vcx, "the main panel opens", |vcx| {
        bounds(vcx, "tab-Team").is_some()
    });
    click(&mut vcx, "tab-Settings");
    let main = vcx.read(|cx| {
        window
            .read(cx)
            .unwrap()
            .project(project)
            .unwrap()
            .main
            .clone()
    });
    let settings = vcx
        .read(|cx| main.read(cx).tab_view::<SettingsView>(Tab::Settings))
        .expect("the Settings tab");
    let dm = part(&vcx, &settings);
    assert_eq!(vcx.read(|cx| dm.read(cx).shown().connected), None);

    // Pick OpenAI, paste the key, connect.
    scroll_down(&mut vcx);
    click(&mut vcx, "decision-provider-openai");
    assert_eq!(
        vcx.read(|cx| dm.read(cx).chosen()),
        DecisionProvider::OpenAi
    );
    click(&mut vcx, "decision-key");
    vcx.simulate_input(KEY);
    scroll_down(&mut vcx);
    click(&mut vcx, "decision-connect");
    pump_until(&mut vcx, "the model is connected", |vcx| {
        vcx.read(|cx| dm.read(cx).shown().connected) == Some(DecisionProvider::OpenAi)
    });
    assert_eq!(vcx.read(|cx| dm.read(cx).outcome().cloned()), None);
    assert_eq!(
        vcx.read(|cx| dm.read(cx).key_text(cx)),
        "",
        "the field is emptied once the daemon has the key"
    );

    // The daemon holds it as a credential for OpenAI's host alone, which no project may use.
    let (setting, credential) = client.with_state(|s| {
        let setting = s.daemon.settings().decision_model.expect("connected");
        let credential = s
            .daemon
            .credentials()
            .find(|c| c.credential.reference == setting.credential)
            .map(|c| c.credential.clone())
            .expect("its credential");
        (setting, credential)
    });
    assert_eq!(setting.provider, DecisionProvider::OpenAi);
    assert_eq!(
        credential.scope.hosts,
        BTreeSet::from(["api.openai.com".to_owned()])
    );
    assert!(credential.scope.commands.is_empty());
    assert_eq!(credential.scope.projects, Projects::Only(BTreeSet::new()));
    // The key is in nothing the app holds.
    let held = client.with_state(|s| format!("{:?}", s.daemon));
    assert!(!held.contains(KEY));
    let shown = vcx.read(|cx| format!("{:?}", dm.read(cx).shown()));
    assert!(!shown.contains(KEY));

    // Disconnect: the setting is cleared and the key removed.
    scroll_down(&mut vcx);
    click(&mut vcx, "decision-disconnect");
    pump_until(&mut vcx, "the model is disconnected", |vcx| {
        vcx.read(|cx| dm.read(cx).shown().connected).is_none()
    });
    assert_eq!(vcx.read(|cx| dm.read(cx).outcome().cloned()), None);
    let gone = client.with_state(|s| {
        s.daemon
            .credentials()
            .all(|c| c.credential.reference != setting.credential)
    });
    assert!(gone, "the key is removed with it");
}
