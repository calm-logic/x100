//! The integration tools' rows: listing and linking change nothing outside, and a deploy changes
//! what is live at a public address, after creating a repository and pushing to it if needed.

mod common;

use common::*;
use x100_core::action::{ActionKind, AskMeAbout, Risk};
use x100_core::tool::ToolCall;

/// A deploy is High whether or not it creates a repository and pushes to it first, since both
/// change what is live at a public address and send the code off the machine: at "High risk only"
/// and up it is held, at "Nothing" it is not. Listing and linking change nothing outside and are
/// Low.
#[test]
fn integrations_deploy_at_high_and_list_and_link_at_low() {
    let (mut state, f) = fern(AskMeAbout::HighRiskOnly);
    for call in [
        ToolCall::ListIntegrations {},
        ToolCall::LinkProject {
            homebox_project: "fernleaf".into(),
            environment: None,
        },
    ] {
        let r = ruling(&state, &tool(f.ceo, call));
        assert_eq!(r.classification().risk, Risk::Low);
        assert!(!r.is_held());
    }
    let deploy = ToolCall::Deploy { environment: None };
    let first = ruling(&state, &tool(f.ceo, deploy.clone()));
    assert_eq!(
        rule_of(&first).as_deref(),
        Some("tool.deploy.publish_first")
    );
    let c = first.classification();
    assert_eq!(
        (c.kind, c.risk),
        (ActionKind::DeployToProduction, Risk::High)
    );
    assert!(first.is_held());
    let link = x100_core::integration::HomeboxLink {
        instance: "http://127.0.0.1:7765".into(),
        repo_full_name: "builder/fernleaf".into(),
        project_id: "7".into(),
        project_name: "fernleaf".into(),
        environment: "production".into(),
    };
    push(
        &mut state,
        x100_core::party::Actor::Builder,
        x100_core::event::ProjectEvent::SettingChanged(x100_core::project::SettingChange::Homebox(
            Some(link),
        )),
    );
    let linked = ruling(&state, &tool(f.ceo, deploy.clone()));
    assert_eq!(rule_of(&linked).as_deref(), Some("tool.deploy"));
    let c = linked.classification();
    assert_eq!(
        (c.kind, c.risk),
        (ActionKind::DeployToProduction, Risk::High)
    );
    assert!(linked.is_held());
    for level in [AskMeAbout::Nothing, AskMeAbout::MediumAndUp] {
        set_level(&mut state, level);
        let r = ruling(&state, &tool(f.ceo, deploy.clone()));
        assert_eq!(r.is_held(), level != AskMeAbout::Nothing, "{level:?}");
    }
}
