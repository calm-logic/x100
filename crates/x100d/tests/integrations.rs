//! M12I's check through the daemon as a child process, on the fake harness and the fake clock,
//! with stand-ins for everything outside: Homebox's API (`standin/homebox.rs`) on a loopback port
//! as a local Homebox, and over TLS as `homebox.x100.test` for one elsewhere; a `docker` script
//! that finds that local Homebox and prints a minted key; a `gh` script that is logged in and
//! makes bare repositories in a directory that stands for GitHub; and GitHub's device flow on the
//! same TLS port.
//!
//! - "Use local" is offered first when the local Homebox is found and connects with no key typed;
//!   without one, the builder gives a host and a key, and a wrong key, a read key and plain HTTP
//!   to another machine are refused. GitHub's sign-in is offered only with an OAuth app's id.
//! - No key or token is in any file under the test's directory, any event, the state, a
//!   transcript or a reply, in the encodings a careless program might use.
//! - A project is linked from the protocol, as Settings does, and by the CEO by name from the
//!   chat; a name that fits none answers with the choices. The link survives a restart.
//! - A deploy asked for in the chat is held at "High risk only", runs on Approve, and the CEO is
//!   told the address; a failing one and a crash-looping one say why. Without GitHub a deploy is
//!   refused with the reason. A project with no repository is created, pushed, added, linked and
//!   deployed. A change asked for with its deploy is deployed once the CEO's report has put it on
//!   main, not before. With `auto_deploy` on x100 waits for the pushed commit's deployment; off, it asks
//!   for one; neither deploys twice. A deploy cut off by the daemon stopping fails when it starts
//!   again, and the CEO is told.

mod common;
#[path = "integrations/rig.rs"]
mod rig;
mod standin;

use common::WAIT;
use rig::*;
use standin::homebox::{ACCOUNT, DEVICE_TOKEN, DOMAIN, Spec, head};
use x100_core::action::ActionState;
use x100_core::credential::Projects;
use x100_core::integration::{GitHubWay, HomeboxWay};
use x100_proto::{ErrorCode, GitHubConnecting, GitHubMethod, Integration};

#[tokio::test(flavor = "multi_thread")]
async fn use_local_is_offered_first_and_connects_with_no_key_typed() {
    let mut r = Rig::start(Has {
        local: true,
        gh: false,
        client_id: false,
    })
    .await;
    let listed = r.client.list_integrations().await.unwrap();
    assert_eq!(listed.homebox.local, Some(r.hb.address()));
    assert_eq!(listed.homebox.default_way, HomeboxWay::Local);
    assert_eq!(listed.homebox.connected, None);
    assert!(listed.github.ways.is_empty() && listed.github.default_way.is_none());
    let (why, _) = code(
        r.client
            .connect_github(GitHubMethod::DeviceFlow)
            .await
            .unwrap_err(),
    );
    assert_eq!(why, ErrorCode::Unavailable, "no OAuth app, no sign-in");

    let connected = r.client.connect_local_homebox().await.unwrap().connection;
    assert_eq!(connected.way, HomeboxWay::Local);
    assert_eq!(connected.address, r.hb.address());
    assert_eq!(connected.primary_domain.as_deref(), Some(DOMAIN));
    assert_eq!(connected.github_accounts, [ACCOUNT]);
    // The one fixed command, run once.
    assert_eq!(
        r.ran("docker")
            .iter()
            .filter(|l| l.starts_with("exec"))
            .collect::<Vec<_>>(),
        ["exec -w /srv homebox-admin python -c"]
    );
    // The key went to the stand-in in the header, and is kept for that port alone, for no
    // project.
    let me =
        r.hb.received()
            .into_iter()
            .find(|q| q.path == "/api/auth/me")
            .unwrap();
    assert_eq!(me.authorization, Some(format!("Bearer {LOCAL_KEY}")));
    let credential = r.client.with_state(|s| {
        s.daemon
            .credentials()
            .find(|c| c.credential.reference == connected.credential)
            .map(|c| c.credential.clone())
            .unwrap()
    });
    let authority = format!("127.0.0.1:{}", r.hb.port);
    assert_eq!(credential.scope.hosts, [authority].into());
    assert_eq!(
        credential.scope.projects,
        Projects::Only(Default::default())
    );
    let projects = r.client.homebox_projects().await.unwrap();
    assert!(projects.projects.is_empty());
    no_key_anywhere(&r, &[LOCAL_KEY]).await;

    // It is still connected after a restart, and Disconnect removes the key.
    r.restart().await;
    assert!(r.client.homebox_projects().await.is_ok());
    r.client
        .disconnect_integration(Integration::Homebox)
        .await
        .unwrap();
    let left = r.client.with_state(|s| {
        (
            s.daemon.settings().homebox.clone(),
            s.daemon.credentials().count(),
        )
    });
    assert_eq!(left, (None, 0));
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_local_homebox_the_builder_gives_a_host_and_a_key() {
    let mut r = Rig::start(Has {
        local: false,
        gh: false,
        client_id: true,
    })
    .await;
    let listed = r.client.list_integrations().await.unwrap();
    assert_eq!(listed.homebox.local, None);
    assert_eq!(listed.homebox.default_way, HomeboxWay::Address);
    assert_eq!(listed.github.ways, [GitHubMethod::DeviceFlow]);
    let (why, _) = code(r.client.connect_local_homebox().await.unwrap_err());
    assert_eq!(why, ErrorCode::Rejected);
    assert!(r.ran("docker").is_empty(), "nothing was minted");

    let remote = format!("https://{REMOTE_HOST}");
    for (address, key, says) in [
        (
            remote.as_str(),
            "hbk_not_one_this_homebox_made_00",
            "refused the key",
        ),
        (remote.as_str(), READ_KEY, "can only read"),
        ("http://homebox.x100.test", EDIT_KEY, "plain HTTP"),
    ] {
        let (why, message) = code(r.client.connect_homebox_at(address, key).await.unwrap_err());
        assert_eq!(why, ErrorCode::Rejected, "{message}");
        assert!(message.contains(says), "{message}");
    }
    let connected = r
        .client
        .connect_homebox_at(&remote, EDIT_KEY)
        .await
        .unwrap()
        .connection;
    assert_eq!(connected.way, HomeboxWay::Address);
    assert_eq!(connected.address, remote);
    let seen = r.hb.received();
    let me = seen.iter().rfind(|q| q.path == "/api/auth/me").unwrap();
    assert_eq!(me.host, REMOTE_HOST);
    assert_eq!(me.authorization, Some(format!("Bearer {EDIT_KEY}")));
    let hosts = r.client.with_state(|s| {
        s.daemon
            .credentials()
            .map(|c| c.credential.scope.hosts.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        hosts,
        [[REMOTE_HOST.to_owned()].into()],
        "refused keys are not kept"
    );

    // GitHub's sign-in, on its stand-in: a code for the builder, then the token in the vault.
    let approve = r
        .client
        .connect_github(GitHubMethod::DeviceFlow)
        .await
        .unwrap();
    assert_eq!(
        approve,
        GitHubConnecting::Approve {
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://github.com/login/device".into(),
        }
    );
    let github = r
        .client
        .wait_for(WAIT, |s| s.daemon.settings().github.clone())
        .await
        .unwrap();
    assert_eq!(github.account, ACCOUNT);
    assert!(matches!(github.way, GitHubWay::DeviceFlow { .. }));
    // Pushing with that token is not built: a deploy says so instead of trying.
    let p = project(&r, "signed-in").await;
    let (why, message) = code(r.client.deploy(p.id, None).await.unwrap_err());
    assert_eq!(why, ErrorCode::WrongState);
    assert!(message.contains("local gh"), "{message}");
    no_key_anywhere(&r, &[EDIT_KEY, READ_KEY, DEVICE_TOKEN]).await;
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_is_linked_from_settings_and_by_the_ceo_by_name_and_the_link_survives_a_restart()
{
    let mut r = Rig::start(Has {
        local: true,
        gh: false,
        client_id: false,
    })
    .await;
    r.hb.project(Spec::managed("foobar", true));
    r.hb.project(Spec::managed("shop", true));
    r.hb.project(Spec {
        managed: false,
        ..Spec::managed("draft", true)
    });
    r.connect_local().await;
    let p = project(&r, "linked").await;

    // From the protocol, as Settings does: by name, and refused for an unmanaged project.
    let link = r.client.link_homebox(p.id, "shop", None).await.unwrap();
    assert_eq!(link.repo_full_name, format!("{ACCOUNT}/shop"));
    assert_eq!(link.environment, "production");
    assert_eq!(link.instance, r.hb.address());
    let (_, message) = code(
        r.client
            .link_homebox(p.id, "draft", None)
            .await
            .unwrap_err(),
    );
    assert!(message.contains("does not manage"), "{message}");
    r.client.unlink_homebox(p.id).await.unwrap();
    r.client
        .wait_for(WAIT, |s| {
            s.projects
                .get(&p.id)?
                .project()?
                .homebox
                .is_none()
                .then_some(())
        })
        .await
        .unwrap();

    // By the CEO, from the chat: a name that fits none gets the choices, then the right one.
    say(&r, &p, "LINK\nHomebox: foobaz").await;
    let choices = ceo_saw(&r, &p, "Choose one of").await;
    assert!(choices.contains("foobar (builder/foobar)"), "{choices}");
    say(&r, &p, "LINK\nHomebox: FooBar").await;
    let link = r
        .client
        .wait_for(WAIT, |s| s.projects.get(&p.id)?.project()?.homebox.clone())
        .await
        .unwrap();
    assert_eq!(link.project_name, "foobar");

    r.restart().await;
    let after = r
        .client
        .wait_for(WAIT, |s| s.projects.get(&p.id)?.project()?.homebox.clone())
        .await
        .unwrap();
    assert_eq!(after, link);
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn without_github_a_deploy_is_refused_with_the_reason() {
    let mut r = Rig::start(Has {
        local: true,
        gh: false,
        client_id: false,
    })
    .await;
    r.hb.project(Spec::managed("foobar", false));
    r.connect_local().await;
    let p = project(&r, "local-only").await;
    commit(&p, "<h1>hello</h1>");
    r.client.link_homebox(p.id, "foobar", None).await.unwrap();

    say(&r, &p, "DEPLOY").await;
    let refused = ceo_saw(&r, &p, "GitHub is not connected").await;
    assert!(refused.contains("Settings"), "{refused}");
    assert!(
        state(&r, &p)
            .actions()
            .all(|a| a.state != ActionState::Held),
        "nothing is put to the builder that could not be done"
    );
    let (why, message) = code(r.client.deploy(p.id, None).await.unwrap_err());
    assert_eq!(why, ErrorCode::WrongState);
    assert!(message.contains("GitHub is not connected"), "{message}");
    assert_eq!(
        head(&r.remotes, "builder/foobar"),
        None,
        "nothing was pushed"
    );
    assert_eq!(r.hb.deploy_calls("foobar"), 0);
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_from_the_chat_waits_for_approve_and_the_ceo_is_told_the_address_or_why_not() {
    let mut r = Rig::start(Has {
        local: true,
        gh: true,
        client_id: false,
    })
    .await;
    r.hb.project(Spec::managed("foobar", false));
    r.hb.project(Spec {
        fails: true,
        ..Spec::managed("broken", false)
    });
    r.hb.project(Spec {
        crashes: true,
        ..Spec::managed("crashy", false)
    });
    r.connect_local().await;
    let listed = r.client.list_integrations().await.unwrap();
    assert_eq!(listed.github.default_way, Some(GitHubMethod::LocalGh));
    let connected = r
        .client
        .connect_github(GitHubMethod::LocalGh)
        .await
        .unwrap();
    assert_eq!(
        connected,
        GitHubConnecting::Connected {
            account: ACCOUNT.into()
        }
    );
    let p = project(&r, "site").await;
    let page = commit(&p, "<h1>hello</h1>");
    r.client.link_homebox(p.id, "foobar", None).await.unwrap();

    say(&r, &p, "DEPLOY").await;
    let held = deploy_action(&r, &p, ActionState::Held, &[]).await;
    assert!(
        held.classification
            .as_ref()
            .unwrap()
            .why
            .contains("tool.deploy"),
        "{:?}",
        held.classification
    );
    assert_eq!(
        head(&r.remotes, "builder/foobar"),
        None,
        "nothing before Approve"
    );
    approve(&r, &p, held.action.id).await;
    deploy_action(&r, &p, ActionState::CarriedOut, &[]).await;
    let mut before = vec![held.action.id];
    let told = ceo_saw(&r, &p, "live at https://foobar.box.x100.test").await;
    assert!(told.contains("Approved"), "{told}");
    assert_eq!(head(&r.remotes, "builder/foobar"), Some(page.clone()));
    assert_eq!(
        r.hb.deploy_calls("foobar"),
        1,
        "auto_deploy is off: asked once"
    );
    assert_eq!(r.hb.deployments_of("foobar", &page).len(), 1);

    // A deploy that fails says why, with Homebox's error and the end of its log; a container
    // that does not answer is a failure too.
    for (name, why) in [
        ("broken", "Missing script"),
        ("crashy", "crashing over and over"),
    ] {
        r.client.link_homebox(p.id, name, None).await.unwrap();
        say(&r, &p, "DEPLOY").await;
        let held = deploy_action(&r, &p, ActionState::Held, &before).await;
        approve(&r, &p, held.action.id).await;
        deploy_action(&r, &p, ActionState::Failed, &before).await;
        before.push(held.action.id);
        let told = ceo_saw(&r, &p, why).await;
        assert!(told.contains("The deploy failed"), "{told}");
    }

    // At "Nothing" it is not held: it runs at once, and the result still reaches the CEO.
    r.client
        .request(x100_proto::ChangeProjectSetting {
            project: p.id,
            change: x100_core::project::SettingChange::AskMeAbout(
                x100_core::action::AskMeAbout::Nothing,
            ),
        })
        .await
        .unwrap();
    let again = commit(&p, "<h1>again</h1>");
    r.client.link_homebox(p.id, "foobar", None).await.unwrap();
    say(&r, &p, "DEPLOY").await;
    let done = deploy_action(&r, &p, ActionState::CarriedOut, &before).await;
    assert!(done.decided.is_none(), "never held");
    let told = ceo_saw(&r, &p, "carried out in the background is done").await;
    assert!(
        told.contains("live at https://foobar.box.x100.test"),
        "{told}"
    );
    assert_eq!(head(&r.remotes, "builder/foobar"), Some(again));
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_with_no_repository_is_published_added_linked_and_deployed() {
    let mut r = Rig::start(Has {
        local: true,
        gh: true,
        client_id: false,
    })
    .await;
    r.connect_local().await;
    r.client
        .connect_github(GitHubMethod::LocalGh)
        .await
        .unwrap();
    let p = project(&r, "fresh").await;
    let page = commit(&p, "<h1>fresh</h1>");

    say(&r, &p, "DEPLOY").await;
    let held = deploy_action(&r, &p, ActionState::Held, &[]).await;
    assert!(
        held.classification
            .as_ref()
            .unwrap()
            .why
            .contains("tool.deploy.publish_first"),
        "{:?}",
        held.classification
    );
    approve(&r, &p, held.action.id).await;
    deploy_action(&r, &p, ActionState::CarriedOut, &[]).await;
    ceo_saw(&r, &p, "live at https://fresh.box.x100.test").await;
    assert!(
        r.ran("gh")
            .contains(&"repo create builder/fresh --private --disable-wiki".to_owned()),
        "{:?}",
        r.ran("gh")
    );
    assert_eq!(head(&r.remotes, "builder/fresh"), Some(page.clone()));
    assert!(r.hb.managed("fresh"));
    // Adding queued the first deploy, which was watched, not asked for again.
    assert_eq!(r.hb.deploy_calls("fresh"), 0);
    assert_eq!(r.hb.deployments_of("fresh", &page).len(), 1);
    let s = state(&r, &p);
    let project = s.project().unwrap();
    assert_eq!(project.repository.as_deref(), Some("builder/fresh"));
    assert_eq!(
        project.homebox.as_ref().map(|l| l.repo_full_name.as_str()),
        Some("builder/fresh")
    );
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_asked_with_its_deploy_is_deployed_only_once_it_is_on_main() {
    let mut r = Rig::start(Has {
        local: true,
        gh: true,
        client_id: false,
    })
    .await;
    r.connect_local().await;
    r.client
        .connect_github(GitHubMethod::LocalGh)
        .await
        .unwrap();
    let p = project(&r, "changed").await;
    commit(&p, "<h1>before</h1>");

    say(&r, &p, "CHANGE AND DEPLOY").await;
    let refused = ceo_saw(&r, &p, "not on the project's main branch yet").await;
    assert!(refused.contains("`report` your task first"), "{refused}");
    let told = ceo_saw(&r, &p, "If the builder asked for a deploy").await;
    assert!(told.contains("call `deploy` now"), "{told}");
    let held = deploy_action(&r, &p, ActionState::Held, &[]).await;
    approve(&r, &p, held.action.id).await;
    deploy_action(&r, &p, ActionState::CarriedOut, &[]).await;
    let live = ceo_saw(&r, &p, "Address: https://changed.box.x100.test").await;
    assert!(
        live.contains("live at https://changed.box.x100.test"),
        "{live}"
    );
    let pushed = head(&r.remotes, "builder/changed").expect("main was pushed");
    let shipped = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(r.remotes.join("builder/changed.git"))
        .args(["show", &format!("{pushed}:index.html")])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&shipped.stdout),
        "<h1>changed</h1>",
        "the change itself was deployed"
    );
    assert_eq!(r.hb.deployments_of("changed", &pushed).len(), 1);
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn both_auto_deploy_settings_follow_the_rule_and_never_deploy_twice() {
    let mut r = Rig::start(Has {
        local: true,
        gh: true,
        client_id: false,
    })
    .await;
    r.hb.project(Spec::managed("auto", true));
    r.hb.project(Spec::managed("manual", false));
    r.connect_local().await;
    r.client
        .connect_github(GitHubMethod::LocalGh)
        .await
        .unwrap();
    let p = project(&r, "rules").await;

    // On: the push deploys by webhook, and x100 waits for that deployment.
    let first = commit(&p, "<h1>one</h1>");
    r.client.link_homebox(p.id, "auto", None).await.unwrap();
    let done = r.client.deploy(p.id, None).await.unwrap();
    assert_eq!(done.address.as_deref(), Some("https://auto.box.x100.test"));
    assert_eq!(done.commit, first);
    assert_eq!(r.hb.deploy_calls("auto"), 0);
    assert_eq!(r.hb.deployments_of("auto", &first).len(), 1);
    // Nothing new: what is live is that commit already.
    r.client.deploy(p.id, None).await.unwrap();
    assert_eq!(r.hb.deploy_calls("auto"), 0);
    assert_eq!(r.hb.deployments_of("auto", &first).len(), 1);

    // Off: x100 asks for the deploy, once, and checks the commit.
    let second = commit(&p, "<h1>two</h1>");
    r.client.link_homebox(p.id, "manual", None).await.unwrap();
    let done = r.client.deploy(p.id, None).await.unwrap();
    assert_eq!(done.commit, second);
    assert_eq!(r.hb.deploy_calls("manual"), 1);
    assert_eq!(r.hb.deployments_of("manual", &second).len(), 1);
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_cut_off_by_the_daemon_stopping_fails_when_it_starts_again() {
    let mut r = Rig::start(Has {
        local: true,
        gh: true,
        client_id: false,
    })
    .await;
    r.hb.project(Spec {
        stalls: true,
        ..Spec::managed("slow", false)
    });
    r.connect_local().await;
    r.client
        .connect_github(GitHubMethod::LocalGh)
        .await
        .unwrap();
    let p = project(&r, "cut-off").await;
    commit(&p, "<h1>slow</h1>");
    r.client.link_homebox(p.id, "slow", None).await.unwrap();
    r.client
        .request(x100_proto::ChangeProjectSetting {
            project: p.id,
            change: x100_core::project::SettingChange::AskMeAbout(
                x100_core::action::AskMeAbout::Nothing,
            ),
        })
        .await
        .unwrap();
    say(&r, &p, "DEPLOY").await;
    let going = deploy_action(&r, &p, ActionState::Allowed, &[]).await;
    // Under way: asked for, and its deployment still queued.
    let until = std::time::Instant::now() + common::WAIT;
    while r.hb.deploy_calls("slow") == 0 {
        assert!(
            std::time::Instant::now() < until,
            "the deploy never asked Homebox"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    r.restart().await;
    let ended = deploy_action(&r, &p, ActionState::Failed, &[]).await;
    assert_eq!(ended.action.id, going.action.id);
    assert!(ended.error.unwrap().contains("the daemon stopped"));
    ceo_saw(&r, &p, "the daemon stopped before").await;
    r.daemon.terminate();
}
