//! Settings' Integrations part and a project's Homebox project, against a real daemon on the fake
//! clock without a display, with the daemon's tests' Homebox stand-in on loopback and stand-in
//! `docker` and `gh` programs that find it and are logged in.
//!
//! - Homebox offers "Use local" first, since one is found; it asks before anything is made, and
//!   Cancel makes nothing. Confirmed, the daemon mints a key with its one command and connects.
//! - GitHub offers the local `gh` first and connects with it.
//! - The project's Homebox project lists the instance's managed projects; picking one links the
//!   project and Unlink removes the link.
//! - Disconnected, Homebox's other way is one click away: an address and a key, which is emptied
//!   from its field once the daemon has it.

mod common;
#[path = "../../x100d/tests/standin/homebox.rs"]
mod homebox;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::*;
use gpui::{Entity, VisualTestContext};
use homebox::{ACCOUNT, HomeboxStandIn, Spec};
use x100_app::settings::{HomeboxPart, IntegrationsPart, SettingsView};
use x100_app::tabs::Tab;
use x100_core::integration::{GitHubWay, HomeboxWay};
use x100_proto::GitHubMethod;

/// What the stand-in `docker` prints as the minted key, and a key pasted later; invented here.
const MINTED: &str = "hbk_app_test_minted_key_7d3e1a9c";
const PASTED: &str = "hbk_app_test_pasted_key_2b8f6c4e";

fn script(path: &Path, body: String) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Scrolls the page down, as the wheel would, until `selector` is drawn inside the window.
fn reach(vcx: &mut VisualTestContext, selector: &'static str) {
    for _ in 0..20 {
        let found = bounds(vcx, selector);
        if let Some(b) = found
            && b.origin.y > gpui::px(0.)
            && b.origin.y + b.size.height < gpui::px(880.)
        {
            return;
        }
        // A part drawn once its answer arrives can appear above a page already scrolled past it,
        // so the wheel turns toward it rather than always down.
        let step = match found {
            Some(b) if b.origin.y <= gpui::px(0.) => 300.,
            _ => -300.,
        };
        vcx.simulate_event(gpui::ScrollEvent {
            position: gpui::point(gpui::px(900.), gpui::px(500.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(step))),
            ..Default::default()
        });
        pump(vcx);
    }
    panic!("{selector} is never in view");
}

/// A click as two events, each followed by every task that is ready. Something the page reads
/// can land between them and move the control, and GPUI then drops the click, so a move is
/// reported as what it is rather than as a wait that times out.
fn press(vcx: &mut VisualTestContext, selector: &'static str) {
    reach(vcx, selector);
    let down = bounds(vcx, selector).unwrap();
    vcx.simulate_mouse_down(
        down.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    let now = bounds(vcx, selector);
    assert_eq!(
        now.map(|b| b.origin),
        Some(down.origin),
        "{selector} moved between mouse down and up: the page was still filling in"
    );
    vcx.simulate_mouse_up(
        down.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::none(),
    );
    pump(vcx);
}

#[test]
fn homebox_and_github_connect_from_settings_and_a_project_is_linked() {
    let bin = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let remotes = bin.path().join("github");
    std::fs::create_dir_all(&remotes).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let hb = rt.block_on(HomeboxStandIn::start(&remotes));
    hb.key(MINTED, "edit");
    hb.key(PASTED, "edit");
    hb.project(Spec::managed("foobar", true));
    hb.project(Spec::managed("shop", false));
    let log = bin.path().join("docker.log");
    script(
        &bin.path().join("docker"),
        format!(
            "case \"$1\" in\nport) echo 127.0.0.1:{port}; exit 0;;\n\
             exec) echo exec >> '{log}'; echo '{MINTED}'; exit 0;;\nesac\nexit 1\n",
            port = hb.port,
            log = log.display()
        ),
    );
    script(
        &bin.path().join("gh"),
        format!(
            "case \"$1 $2\" in\n'auth status') exit 0;;\n'api user') echo {ACCOUNT}; exit 0;;\n\
             esac\nexit 1\n"
        ),
    );
    let mut run = Run::start_with(&[
        (
            "X100_DOCKER",
            bin.path().join("docker").display().to_string(),
        ),
        ("X100_GH", bin.path().join("gh").display().to_string()),
    ]);
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
    let settings: Entity<SettingsView> = vcx
        .read(|cx| main.read(cx).tab_view::<SettingsView>(Tab::Settings))
        .expect("the Settings tab");
    let part: Entity<IntegrationsPart> = vcx.read(|cx| settings.read(cx).integrations().clone());
    let linked: Entity<HomeboxPart> = vcx.read(|cx| settings.read(cx).homebox().clone());

    // What the daemon found: the local Homebox, offered first, and gh, offered first.
    pump_until(&mut vcx, "the daemon says what it found", |vcx| {
        vcx.read(|cx| part.read(cx).shown().found.is_some())
    });
    assert_eq!(
        vcx.read(|cx| part.read(cx).homebox_way()),
        HomeboxWay::Local
    );
    assert_eq!(
        vcx.read(|cx| part.read(cx).github_way()),
        Some(GitHubMethod::LocalGh)
    );
    assert!(vcx.read(|cx| linked.read(cx).shown().connected.is_none()));

    // "Use local" asks first; Cancel makes nothing.
    press(&mut vcx, "homebox-use-local");
    assert!(vcx.read(|cx| part.read(cx).confirming()));
    press(&mut vcx, "homebox-cancel");
    assert!(!vcx.read(|cx| part.read(cx).confirming()));
    assert!(
        !log.exists(),
        "nothing was run before the builder confirmed"
    );
    press(&mut vcx, "homebox-use-local");
    press(&mut vcx, "homebox-confirm");
    pump_until(&mut vcx, "Homebox is connected", |vcx| {
        vcx.read(|cx| part.read(cx).shown().homebox.is_some())
    });
    let connected = client.with_state(|s| s.daemon.settings().homebox.clone().unwrap());
    assert_eq!(connected.way, HomeboxWay::Local);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "exec\n", "one mint");
    assert_eq!(
        vcx.read(|cx| part.read(cx).homebox_outcome().cloned()),
        None
    );

    // Connected, the project's Homebox project asks for the instance's projects and lists them
    // above Integrations, which moves GitHub's Connect down; it is pressed once they are there.
    pump_until(&mut vcx, "the Homebox projects are listed", |vcx| {
        vcx.read(|cx| linked.read(cx).shown().projects.is_some())
    });

    // GitHub through the local gh.
    press(&mut vcx, "github-connect");
    pump_until(&mut vcx, "GitHub is connected", |vcx| {
        vcx.read(|cx| part.read(cx).shown().github.is_some())
    });
    let github = client.with_state(|s| s.daemon.settings().github.clone().unwrap());
    assert_eq!(
        (github.way, github.account.as_str()),
        (GitHubWay::LocalGh, ACCOUNT)
    );

    // The project's Homebox project: a click on one of the instance's managed projects links
    // it, and Unlink.
    press(&mut vcx, "homebox-project-shop");
    let link = pump_for(&mut vcx, &client, ("the project is linked", WAIT), |s| {
        s.projects.get(&project)?.project()?.homebox.clone()
    });
    assert_eq!(link.repo_full_name, format!("{ACCOUNT}/shop"));
    assert_eq!(
        vcx.read(|cx| linked.read(cx).shown().link.clone()),
        Some(link)
    );
    press(&mut vcx, "homebox-unlink");
    pump_for(&mut vcx, &client, ("the project is unlinked", WAIT), |s| {
        s.projects
            .get(&project)?
            .project()?
            .homebox
            .is_none()
            .then_some(())
    });

    // Disconnect, and the other way is one click away: the address and a key.
    press(&mut vcx, "homebox-disconnect");
    pump_until(&mut vcx, "Homebox is disconnected", |vcx| {
        vcx.read(|cx| part.read(cx).shown().homebox.is_none())
    });
    // The daemon removes the key in a second event. It leaves Credentials, at the end of the
    // page, which moves what is above when the page is scrolled to its end.
    pump_until(&mut vcx, "Homebox's key is removed", |_| {
        client.with_state(|s| {
            !s.daemon
                .credentials()
                .any(|c| c.credential.reference == connected.credential)
        })
    });
    press(&mut vcx, "homebox-way-address");
    press(&mut vcx, "homebox-address");
    vcx.simulate_input(&hb.address());
    press(&mut vcx, "homebox-key");
    vcx.simulate_input(PASTED);
    press(&mut vcx, "homebox-connect");
    pump_until(&mut vcx, "Homebox is connected with the key", |vcx| {
        vcx.read(|cx| part.read(cx).shown().homebox.is_some())
    });
    let again = client.with_state(|s| s.daemon.settings().homebox.clone().unwrap());
    assert_eq!(again.way, HomeboxWay::Address);
    assert_eq!(
        vcx.read(|cx| part.read(cx).key_text(cx)),
        "",
        "the field is emptied once the daemon has the key"
    );
    for key in [MINTED, PASTED] {
        assert!(
            !client
                .with_state(|s| format!("{:?}", s.daemon))
                .contains(key)
        );
        assert!(
            !vcx.read(|cx| format!("{:?}", part.read(cx).shown()))
                .contains(key)
        );
    }
    drop(rt);
}
