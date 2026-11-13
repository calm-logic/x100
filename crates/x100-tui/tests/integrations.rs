//! Settings' Integrations and the project's Homebox project in the terminal, against a real daemon
//! on the fake clock, with the daemon's tests' Homebox stand-in on loopback and stand-in `docker`
//! and `gh` programs that find it and are logged in, as the app's test has them; `gh` also makes
//! repositories in a directory that stands for GitHub.
//!
//! - Homebox offers "Use local" first, since one is found; it asks before anything is made, and
//!   No makes nothing. Yes, the daemon mints a key with its one command and connects.
//! - GitHub offers the local `gh` and connects with it.
//! - The project's Homebox project lists the instance's managed projects; Enter on one links the
//!   project, Deploy now pushes its main branch there and deploys it, and Unlink removes the link.
//! - Disconnected, the other way is an address and a key, typed masked; the key is drawn nowhere
//!   and held nowhere but the daemon's vault. GitHub disconnects too.
//!
//! What the screen says is compared with what the client holds, which is what the daemon wrote.

mod common;
#[path = "../../x100d/tests/standin/homebox.rs"]
mod homebox;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::*;
use homebox::{ACCOUNT, HomeboxStandIn, Spec, head};
use ratatui::crossterm::event::KeyCode;
use x100_core::integration::{GitHubWay, HomeboxWay};
use x100_core::settings::DaemonSettings;
use x100_proto::{CreateProject, GitHubMethod, Integration};
use x100_tui::app::Tab;
use x100_tui::page::{Setting, Target};
use x100_tui::remember::Remember;

/// What the stand-in `docker` prints as the minted key, and a key pasted later; invented here.
const MINTED: &str = "hbk_tui_test_minted_key_4c1e9b7a";
const PASTED: &str = "hbk_tui_test_pasted_key_8e2d5f3c";

/// No turn here does anything but answer.
const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
repeat = true
steps = [{ text = "noted" }]
"##;

fn script(path: &Path, body: String) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=test", "-c", "user.email=test@x100.test"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn setting(s: Setting) -> Target {
    Target::Setting(s)
}

#[test]
fn homebox_and_github_connect_from_settings_and_a_project_is_linked_and_deployed() {
    let bin = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let remotes = bin.path().join("github");
    std::fs::create_dir_all(remotes.join(ACCOUNT)).unwrap();
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
             'auth git-credential') exit 0;;\nesac\nexit 1\n"
        ),
    );
    let run = Run::scripted(
        SCRIPT,
        &[
            (
                "X100_DOCKER",
                bin.path().join("docker").display().to_string(),
            ),
            ("X100_GH", bin.path().join("gh").display().to_string()),
            (
                "X100_GITHUB_GIT_BASE",
                format!("file://{}", remotes.display()),
            ),
        ],
    );
    let client = run.client.clone();
    let dir = run.daemon.dir.path().join("site");
    std::fs::create_dir_all(&dir).unwrap();
    let project = run.rt.block_on(async {
        let project = client
            .request(CreateProject {
                dir: Some(dir.to_string_lossy().into_owned()),
                settings: None,
            })
            .await
            .expect("the project is created")
            .project;
        client
            .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|_| ()))
            .await
            .expect("the client holds it");
        project
    });
    let mut screen = run.tui(Remember::none());
    screen.tui.app.open_project(project);
    screen.tui.app.set_tab(Tab::Settings);
    let settings = || -> DaemonSettings { client.with_state(|s| s.daemon.settings().clone()) };

    // What the daemon found: the local Homebox, offered first, and gh.
    let found = run.rt.block_on(client.list_integrations()).unwrap();
    let local = found
        .homebox
        .local
        .clone()
        .expect("the local Homebox is found");
    assert_eq!(found.homebox.default_way, HomeboxWay::Local);
    assert_eq!(found.github.ways, [GitHubMethod::LocalGh]);
    let text = screen.wait_for(&run.rt, "the daemon says what it found", |t| {
        t.contains("Use local")
    });
    let use_local = text.find("Use local").unwrap();
    assert!(use_local < text.find("Host and key").unwrap(), "{text}");
    assert!(
        text.contains(&format!("Homebox is running on this machine, at {local}.")),
        "{text}"
    );
    assert!(text.contains("Homebox  Not connected."), "{text}");
    assert!(text.contains("Use local gh"), "{text}");
    assert!(
        text.contains("Not linked. Connect Homebox under Integrations"),
        "{text}"
    );

    // "Use local" asks first; No makes nothing.
    assert!(screen.select(&setting(Setting::HomeboxLocal)));
    screen.press(KeyCode::Enter);
    let asked = screen.text();
    assert!(
        asked.contains("one command in Homebox's container, homebox-admin"),
        "{asked}"
    );
    screen.type_text("n");
    screen.press(KeyCode::Enter);
    assert!(!log.exists(), "nothing was run before the builder said yes");
    assert!(settings().homebox.is_none());
    screen.press(KeyCode::Enter);
    screen.type_text("y");
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "Homebox is connected", |_| {
        settings().homebox.is_some()
    });
    let connected = settings().homebox.unwrap();
    assert_eq!(connected.way, HomeboxWay::Local);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "exec\n", "one mint");
    let text = screen.wait_for(&run.rt, "the connection is shown", |t| {
        t.contains("Connected to")
    });
    assert!(
        text.contains(&format!(
            "Connected to {}, the Homebox on this machine.",
            connected.address
        )),
        "{text}"
    );

    // GitHub through the local gh.
    assert!(screen.select(&setting(Setting::GitHub(GitHubMethod::LocalGh))));
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "GitHub is connected", |_| {
        settings().github.is_some()
    });
    let github = settings().github.unwrap();
    assert_eq!(
        (github.way, github.account.as_str()),
        (GitHubWay::LocalGh, ACCOUNT)
    );
    let text = screen.wait_for(&run.rt, "GitHub's connection is shown", |t| {
        t.contains(&format!(
            "Connected as {ACCOUNT}, through gh on this machine"
        ))
    });
    assert!(!text.contains("Use local gh"), "{text}");

    // The project's Homebox project: the instance's managed projects, Enter to link.
    let listed = run.rt.block_on(client.homebox_projects()).unwrap();
    let shop = format!("{ACCOUNT}/shop");
    let text = screen.wait_for(&run.rt, "the Homebox projects are listed", |t| {
        t.contains(&shop)
    });
    for p in listed.projects.iter().filter(|p| p.managed) {
        assert!(
            text.contains(&format!("[ ] {}  {}", p.name, p.repo_full_name)),
            "{text}"
        );
    }
    assert!(screen.select(&setting(Setting::LinkHomebox(shop.clone()))));
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "the project is linked", |_| {
        client
            .project(project)
            .and_then(|p| p.project()?.homebox.clone())
            .is_some()
    });
    let link = client
        .project(project)
        .and_then(|p| p.project()?.homebox.clone())
        .unwrap();
    assert_eq!(link.repo_full_name, shop);
    let text = screen.wait_for(&run.rt, "the link is shown", |t| {
        t.contains("Deploys to shop")
    });
    assert!(
        text.contains(&format!(
            "Deploys to {} ({}), {}.",
            link.project_name, link.repo_full_name, link.environment
        )),
        "{text}"
    );
    assert!(text.contains(&format!("[x] shop  {shop}")), "{text}");

    // Deploy now: the project's main branch is pushed to the linked repository and deployed.
    std::fs::write(dir.join("index.html"), "<h1>hello</h1>").unwrap();
    git(&dir, &["add", "index.html"]);
    git(&dir, &["commit", "-q", "-m", "a page"]);
    let page = git(&dir, &["rev-parse", "HEAD"]);
    assert!(screen.select(&setting(Setting::Deploy)));
    screen.press(KeyCode::Enter);
    let text = screen.wait_for(&run.rt, "the deploy is done", |t| t.contains("Deployed "));
    assert!(
        text.contains(&format!("Deployed {}", &page[..12])),
        "{text}"
    );
    assert_eq!(head(&remotes, &shop), Some(page.clone()), "main was pushed");
    assert_eq!(hb.deployments_of("shop", &page).len(), 1, "one deploy");

    // Unlink.
    assert!(screen.select(&setting(Setting::UnlinkHomebox)));
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "the project is unlinked", |t| {
        client
            .project(project)
            .and_then(|p| p.project().map(|p| p.homebox.is_none()))
            == Some(true)
            && t.contains("Not linked. Pick the Homebox project")
    });

    // Disconnect, and connect the other way: the address and a key.
    assert!(screen.select(&setting(Setting::Disconnect(Integration::Homebox))));
    screen.press(KeyCode::Enter);
    screen.type_text("y");
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "Homebox is disconnected", |t| {
        settings().homebox.is_none() && t.contains("Homebox  Not connected.")
    });
    assert!(screen.select(&setting(Setting::HomeboxAddress)));
    screen.press(KeyCode::Enter);
    screen.type_text(&hb.address());
    screen.press(KeyCode::Enter);
    screen.type_text(PASTED);
    let typing = screen.text();
    assert!(!typing.contains(PASTED), "the key is drawn:\n{typing}");
    assert!(
        typing.contains(&"•".repeat(PASTED.chars().count())),
        "{typing}"
    );
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "Homebox is connected with the key", |_| {
        settings().homebox.is_some()
    });
    let again = settings().homebox.unwrap();
    assert_eq!(again.way, HomeboxWay::Address);
    let text = screen.wait_for(&run.rt, "the connection is shown", |t| {
        t.contains("a Homebox elsewhere, with its key")
    });
    assert!(
        text.contains(&format!("Connected to {}", again.address)),
        "{text}"
    );
    assert!(screen.tui.app.project_ui().unwrap().prompt.is_none());
    for key in [MINTED, PASTED] {
        assert!(
            !client
                .with_state(|s| format!("{:?}", s.daemon))
                .contains(key)
        );
        assert!(!text.contains(key));
    }

    // GitHub disconnects too, and the local gh is offered again.
    assert!(screen.select(&setting(Setting::Disconnect(Integration::GitHub))));
    screen.press(KeyCode::Enter);
    screen.type_text("y");
    screen.press(KeyCode::Enter);
    screen.wait_for(&run.rt, "GitHub is disconnected", |t| {
        settings().github.is_none() && t.contains("Use local gh")
    });
    drop(rt);
}
