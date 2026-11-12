//! The Settings tab's view of what the project is: its name, intent and code directory, which the
//! CEO set when it set the project up, read-only but for the name, which the builder may change.

mod common;

use common::*;
use x100_app::settings::{Control, SettingsView};
use x100_app::tabs::Tab;

#[test]
fn settings_show_the_project_and_the_builder_renames_it() {
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
    pump_until(&mut vcx, "the project's identity is shown", |vcx| {
        vcx.read(|cx| settings.read(cx).shown().and_then(|m| m.name.clone()))
            .is_some()
    });
    let shown = vcx.read(|cx| settings.read(cx).shown().cloned()).unwrap();
    let code = run.daemon.projects_root().join("fernleaf");
    assert_eq!(shown.name.as_deref(), Some("Fernleaf"));
    assert_eq!(
        shown.intent.as_deref(),
        Some("A waitlist page for Fernleaf, a plant shop.")
    );
    assert_eq!(shown.code_dir, Some(code.display().to_string()));

    click(&mut vcx, "name-field");
    vcx.simulate_input("Fernleaf Shop");
    click(&mut vcx, "name-set");
    pump_until(&mut vcx, "the new name is everywhere", |vcx| {
        let name = vcx.read(|cx| settings.read(cx).shown().and_then(|m| m.name.clone()));
        let title = vcx.read(|cx| window.read(cx).unwrap().title().to_owned());
        name.as_deref() == Some("Fernleaf Shop") && title == "x100 \u{b7} Fernleaf Shop"
    });
    assert_eq!(
        vcx.read(|cx| settings.read(cx).outcome(Control::Name).cloned()),
        None
    );
    // Renaming moves nothing.
    let p = client.project(project).unwrap();
    assert_eq!(
        p.project().unwrap().code_dir.as_deref(),
        Some(code.as_path())
    );
    assert!(code.join(".git").exists());
}
