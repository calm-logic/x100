//! M12's check through the daemon as a child process, on the fake harness and the fake clock.
//!
//! The daemon's proxy sends a few hosts to a test server on loopback and refuses every other one.
//! The CEO's driver tries requests out with `curl` in its sandbox, and calls `write_spec`, x100's
//! own tool for a file the builder reads. A production deploy is a POST to a hosting service's
//! API; the gate's rules rate it High. No decision model is connected here, so a write to a host
//! no rule knows is held without asking anything; `decision.rs` connects one.
//!
//! - At "High risk only", a read from a registry and `write_spec` are carried out, the deploy is
//!   held and nothing reaches the host. Approve, sent after the agent's driver was replaced,
//!   reaches the new driver, whose retry goes through once and whose second retry is held again.
//!   Deny drops the next deploy and the agent is told. A write to an unknown host is held.
//! - At "Nothing" neither is held; at "Every decision" both are, and an approved `write_spec` is
//!   carried out by the daemon.
//! - A held action is still held after the daemon restarts, and can be decided.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{DaemonProc, Env, WAIT, create_project};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use x100_client::{Client, ClientError};
use x100_core::action::{
    ActionKind, ActionState, AskMeAbout, ClassifiedBy, Decision, Request, Risk,
};
use x100_core::event::ProjectEvent;
use x100_core::fold::{ActionRecord, ProjectState};
use x100_core::id::{ActionId, AgentId, DriverId, ProjectId};
use x100_core::project::SettingChange;
use x100_core::spec::SpecKind;
use x100_core::transcript::Item;
use x100_proto::{
    ChangeProjectSetting, DecideAction, ErrorCode, LogId, LogPage, Page, ReadLog, ReadTranscript,
    StopDriver,
};

const DEPLOY_HOST: &str = "api.vercel.com";
const REGISTRY_HOST: &str = "registry.npmjs.org";
const UNKNOWN_HOST: &str = "mail.x100.test";

const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "DEPLOY" }
repeat = true
steps = [
  { run = "echo {{input:Port}} > port.txt; curl -s -o /dev/null -w '%{http_code}\n' -X POST http://api.vercel.com:{{input:Port}}/v13/deployments >> deploy.txt" },
  { text = "deploy tried" },
]

[[turn]]
when = { input_contains = "FETCH" }
repeat = true
steps = [
  { run = "curl -s -o /dev/null -w '%{http_code}\n' http://registry.npmjs.org:{{input:Port}}/left-pad >> fetch.txt; curl -s -o /dev/null -w '%{http_code}\n' --noproxy '' http://127.0.0.1:9/ >> loopback.txt" },
]

[[turn]]
when = { input_contains = "MAIL" }
repeat = true
steps = [
  { run = "curl -s -o /dev/null -w '%{http_code}\n' -X POST http://mail.x100.test:{{input:Port}}/send >> mail.txt" },
]

[[turn]]
when = { input_contains = "SPEC" }
repeat = true
steps = [{ call = { write_spec = { spec = "brd", status = "draft", body = "# BRD\nGated." } } }]

# An approved request is tried again, twice: the first goes through once, the second is held.
[[turn]]
when = { input_contains = "Send exactly the same request once more" }
repeat = true
steps = [
  { text = "approved: {{input}}" },
  { run = "for i in 1 2; do curl -s -o /dev/null -w '%{http_code}\n' -X POST http://api.vercel.com:$(cat port.txt)/v13/deployments >> deploy.txt; done" },
]

[[turn]]
when = { input_contains = "held action was Denied" }
repeat = true
steps = [{ text = "told: {{input}}" }]

[[turn]]
repeat = true
steps = [{ text = "noted: {{input}}" }]
"##;

/// Counts the requests each host was sent, and answers every one with 200.
type Seen = Arc<Mutex<HashMap<String, usize>>>;

async fn server() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let host = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("host").then(|| v.trim().to_owned())
                    })
                    .unwrap_or_default();
                let host = host.split(':').next().unwrap_or_default().to_owned();
                *log.lock().unwrap().entry(host).or_default() += 1;
                let reply = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, seen)
}

fn sent(seen: &Seen, host: &str) -> usize {
    seen.lock().unwrap().get(host).copied().unwrap_or(0)
}

fn spawn(env: &Env) -> DaemonProc {
    std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
    let mut args = env.daemon_args(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")));
    args.extend(["--fake-clock".into(), "1794000000000".into()]);
    let resolve =
        format!("{DEPLOY_HOST}=127.0.0.1,{REGISTRY_HOST}=127.0.0.1,{UNKNOWN_HOST}=127.0.0.1");
    DaemonProc::spawn_adding(args, &[(x100d::PROXY_RESOLVE_ENV, resolve)])
}

struct Project {
    id: ProjectId,
    ceo: AgentId,
    work: PathBuf,
}

async fn project(client: &Client, env: &Env, name: &str, level: AskMeAbout) -> Project {
    let id = create_project(client, env, name).await;
    client
        .request(ChangeProjectSetting {
            project: id,
            change: SettingChange::AskMeAbout(level),
        })
        .await
        .expect("the level is set");
    client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&id)?.project()?;
            (p.settings.ask_me_about == level).then_some(())
        })
        .await
        .unwrap();
    let ceo = client.project(id).unwrap().project().unwrap().ceo;
    let work = x100_store::Git::new(PathBuf::from(env.project_dir(name))).worktree_path(ceo);
    Project { id, ceo, work }
}

/// Sends the builder's message. The caller waits for what it leads to before saying more, so
/// each message is a turn of its own.
async fn say(client: &Client, p: &Project, body: &str) {
    client.send_chat(p.id, body, vec![], None).await.unwrap();
}

/// Waits until the CEO has handled everything it was given and no turn of its is running, so a
/// driver stopped or a daemon terminated next loses no turn.
async fn settled(client: &Client, p: &Project) {
    client
        .wait_for(WAIT, |s| {
            let st = s.projects.get(&p.id)?;
            let a = st.agent(p.ceo)?;
            let running = a.drivers.iter().any(|d| {
                st.driver(*d)
                    .is_some_and(|r| r.state == x100_core::driver::DriverState::Running)
            });
            (a.inbox.is_empty() && !running).then_some(())
        })
        .await
        .expect("the CEO's turn ended");
}

/// The lines a step wrote to `file` in the worktree, once it has at least `n` of them.
async fn lines(dir: &Path, file: &str, n: usize) -> Vec<String> {
    let until = Instant::now() + WAIT;
    loop {
        let text = std::fs::read_to_string(dir.join(file)).unwrap_or_default();
        let got: Vec<String> = text.lines().map(str::to_owned).collect();
        if got.len() >= n {
            return got;
        }
        assert!(
            Instant::now() < until,
            "{file} has {got:?}, wanted {n} lines"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn egress_to<'a>(s: &'a ProjectState, host: &'a str) -> impl Iterator<Item = &'a ActionRecord> {
    s.actions()
        .filter(move |a| matches!(&a.action.request, Request::Egress { host: h, .. } if h == host))
}

fn tool_writes_spec(a: &ActionRecord) -> bool {
    matches!(&a.action.request, Request::Tool { call, .. } if matches!(call, x100_core::tool::ToolCall::WriteSpec { .. }))
}

async fn state(client: &Client, p: &Project) -> ProjectState {
    client.project(p.id).unwrap()
}

/// Waits until the project holds an action that `f` picks, and returns it.
async fn action(
    client: &Client,
    p: &Project,
    f: impl Fn(&ProjectState) -> Option<ActionRecord>,
) -> ActionRecord {
    client
        .wait_for(WAIT, |s| {
            f(s.projects.get(&p.id)?)
                .filter(|a| !matches!(a.state, ActionState::Proposed | ActionState::Classified))
        })
        .await
        .expect("the action is in the log, ruled on")
}

async fn decide(client: &Client, p: &Project, action: ActionId, decision: Decision) {
    client
        .request(DecideAction {
            project: p.id,
            action,
            decision,
        })
        .await
        .expect("the decision is taken");
}

/// The agent's transcript text that contains `needle`, with the driver that wrote it.
async fn said(client: &Client, p: &Project, needle: &str) -> (DriverId, String) {
    let until = Instant::now() + WAIT;
    loop {
        let entries = client
            .request(ReadTranscript {
                project: p.id,
                agent: p.ceo,
                after: None,
                limit: 10_000,
            })
            .await
            .unwrap()
            .entries;
        let hit = entries.into_iter().find_map(|e| match e.entry.item {
            Item::Text { text } if text.contains(needle) => Some((e.entry.driver, text)),
            _ => None,
        });
        if let Some(hit) = hit {
            return hit;
        }
        assert!(Instant::now() < until, "nothing said {needle:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn at_high_risk_only_a_deploy_waits_for_the_builder_and_the_rest_goes_through() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let env = Env::new();
    let (port, seen) = server().await;
    let mut daemon = spawn(&env);
    let client = env.client().await;
    let p = project(&client, &env, "gated", AskMeAbout::HighRiskOnly).await;

    // A read from a registry is allowed and carried out; the host's own loopback is refused by
    // the proxy, and recorded as such.
    say(&client, &p, &format!("FETCH\nPort: {port}")).await;
    assert_eq!(lines(&p.work, "fetch.txt", 1).await, ["200"]);
    assert_eq!(lines(&p.work, "loopback.txt", 1).await, ["403"]);
    assert_eq!(sent(&seen, REGISTRY_HOST), 1);
    let fetched = action(&client, &p, |s| {
        egress_to(s, REGISTRY_HOST)
            .find(|a| a.state == ActionState::CarriedOut)
            .cloned()
    })
    .await;
    let c = fetched.classification.unwrap();
    assert_eq!((c.risk, c.by), (Risk::Low, ClassifiedBy::Rules));
    let loopback = action(&client, &p, |s| {
        egress_to(s, "127.0.0.1")
            .find(|a| a.state == ActionState::Failed)
            .cloned()
    })
    .await;
    assert!(
        loopback
            .classification
            .unwrap()
            .why
            .contains("egress.refused_by_proxy")
    );

    // `write_spec`, the file the builder reads, is not held.
    say(&client, &p, "SPEC").await;
    let spec = action(&client, &p, |s| {
        s.actions()
            .find(|a| tool_writes_spec(a) && a.state == ActionState::CarriedOut)
            .cloned()
    })
    .await;
    assert_eq!(spec.classification.unwrap().risk, Risk::Low);
    assert!(state(&client, &p).await.spec(SpecKind::Brd).is_some());

    // The deploy is held: the agent is refused at once, and nothing reaches the host.
    say(&client, &p, &format!("DEPLOY\nPort: {port}")).await;
    assert_eq!(lines(&p.work, "deploy.txt", 1).await, ["403"]);
    assert_eq!(sent(&seen, DEPLOY_HOST), 0);
    let held = action(&client, &p, |s| egress_to(s, DEPLOY_HOST).next().cloned()).await;
    assert_eq!(held.state, ActionState::Held);
    let c = held.classification.clone().unwrap();
    assert_eq!(
        (c.kind, c.risk),
        (ActionKind::DeployToProduction, Risk::High)
    );
    let Request::Egress {
        driver: proposer, ..
    } = held.action.request
    else {
        unreachable!()
    };

    // The agent's driver is replaced before the builder answers.
    settled(&client, &p).await;
    client
        .request(StopDriver {
            project: p.id,
            driver: proposer,
        })
        .await
        .unwrap();
    client
        .wait_for(WAIT, |s| {
            let st = s.projects.get(&p.id)?;
            (!st.driver(proposer)?.state.is_live()).then_some(())
        })
        .await
        .unwrap();

    // Approve reaches the new driver, whose retry goes through once; the second is held again.
    decide(&client, &p, held.action.id, Decision::Approved).await;
    let (driver, text) = said(&client, &p, "approved: ").await;
    assert_ne!(
        driver, proposer,
        "the decision reached the driver that replaced it"
    );
    assert!(text.contains(&held.action.id.to_string()), "{text}");
    assert_eq!(lines(&p.work, "deploy.txt", 3).await, ["403", "200", "403"]);
    assert_eq!(sent(&seen, DEPLOY_HOST), 1, "allowed once, not twice");
    let s = state(&client, &p).await;
    assert_eq!(
        s.action(held.action.id).unwrap().state,
        ActionState::CarriedOut
    );
    let again: Vec<_> = egress_to(&s, DEPLOY_HOST)
        .filter(|a| a.action.id != held.action.id)
        .collect();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].state, ActionState::Held);

    // Deny: nothing goes out, the action ends Denied, and the agent is told.
    let denied = again[0].action.id;
    decide(&client, &p, denied, Decision::Denied).await;
    let (_, told) = said(&client, &p, "told: ").await;
    assert!(told.contains(&denied.to_string()), "{told}");
    assert_eq!(sent(&seen, DEPLOY_HOST), 1);
    assert_eq!(
        state(&client, &p).await.action(denied).unwrap().state,
        ActionState::Denied
    );
    // An action that is not held cannot be decided again, and one that does not exist is not
    // found.
    let twice = client
        .request(DecideAction {
            project: p.id,
            action: denied,
            decision: Decision::Approved,
        })
        .await;
    assert!(
        matches!(twice, Err(ClientError::Rpc(ref e)) if e.code == ErrorCode::WrongState),
        "{twice:?}"
    );
    let missing = client
        .request(DecideAction {
            project: p.id,
            action: ActionId::from_u128(7),
            decision: Decision::Approved,
        })
        .await;
    assert!(
        matches!(missing, Err(ClientError::Rpc(ref e)) if e.code == ErrorCode::NotFound),
        "{missing:?}"
    );

    // A write to a host no rule knows, with no decision model connected, is held by the rules,
    // and nothing is asked.
    say(&client, &p, &format!("MAIL\nPort: {port}")).await;
    assert_eq!(lines(&p.work, "mail.txt", 1).await, ["403"]);
    assert_eq!(sent(&seen, UNKNOWN_HOST), 0);
    let mail = action(&client, &p, |s| egress_to(s, UNKNOWN_HOST).next().cloned()).await;
    assert_eq!(mail.state, ActionState::Held);
    let c = mail.classification.unwrap();
    assert_eq!(
        (c.kind, c.risk, c.by),
        (ActionKind::Other, Risk::High, ClassifiedBy::Rules)
    );
    assert!(
        c.why.contains("classifier.not_connected.write"),
        "{}",
        c.why
    );
    let events = log(&client, p.id).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProjectEvent::OneOffCalled(_))),
        "nothing was asked"
    );

    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_level_decides_what_waits() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let env = Env::new();
    let (port, seen) = server().await;
    let mut daemon = spawn(&env);
    let client = env.client().await;

    // At Nothing, neither waits.
    let free = project(&client, &env, "free", AskMeAbout::Nothing).await;
    say(&client, &free, "SPEC").await;
    action(&client, &free, |s| {
        s.actions()
            .find(|a| tool_writes_spec(a) && a.state == ActionState::CarriedOut)
            .cloned()
    })
    .await;
    say(&client, &free, &format!("DEPLOY\nPort: {port}")).await;
    assert_eq!(lines(&free.work, "deploy.txt", 1).await, ["200"]);
    assert_eq!(sent(&seen, DEPLOY_HOST), 1);
    let s = state(&client, &free).await;
    assert!(s.held_actions().next().is_none(), "nothing is held");
    assert!(s.spec(SpecKind::Brd).is_some());

    // At Every decision, both wait.
    let asked = project(&client, &env, "asked", AskMeAbout::EveryDecision).await;
    say(&client, &asked, "SPEC").await;
    action(&client, &asked, |s| {
        s.actions()
            .find(|a| tool_writes_spec(a) && a.state == ActionState::Held)
            .cloned()
    })
    .await;
    say(&client, &asked, &format!("DEPLOY\nPort: {port}")).await;
    assert_eq!(lines(&asked.work, "deploy.txt", 1).await, ["403"]);
    assert_eq!(sent(&seen, DEPLOY_HOST), 1);
    let s = state(&client, &asked).await;
    let spec = s.actions().find(|a| tool_writes_spec(a)).cloned().unwrap();
    assert_eq!(spec.state, ActionState::Held);
    assert!(
        s.spec(SpecKind::Brd).is_none(),
        "a held call has done nothing"
    );
    assert_eq!(
        egress_to(&s, DEPLOY_HOST).next().unwrap().state,
        ActionState::Held
    );

    // Approved, the daemon carries the held call out itself, and the agent is told its result.
    decide(&client, &asked, spec.action.id, Decision::Approved).await;
    let s = state(&client, &asked).await;
    assert_eq!(
        s.action(spec.action.id).unwrap().state,
        ActionState::CarriedOut
    );
    assert!(s.spec(SpecKind::Brd).is_some());
    let (_, noted) = said(&client, &asked, "noted: Your held action was Approved").await;
    assert!(noted.contains(&spec.action.id.to_string()), "{noted}");

    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_action_survives_a_restart_and_can_be_decided() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let env = Env::new();
    let (port, seen) = server().await;
    let mut daemon = spawn(&env);
    let client = env.client().await;
    let p = project(&client, &env, "restarted", AskMeAbout::HighRiskOnly).await;
    say(&client, &p, &format!("DEPLOY\nPort: {port}")).await;
    let held = action(&client, &p, |s| egress_to(s, DEPLOY_HOST).next().cloned()).await;
    assert_eq!(held.state, ActionState::Held);
    settled(&client, &p).await;
    drop(client);
    daemon.terminate();

    let mut daemon = spawn(&env);
    let client = env.client().await;
    let still = client
        .wait_for(WAIT, |s| {
            s.projects.get(&p.id)?.action(held.action.id).cloned()
        })
        .await
        .unwrap();
    assert_eq!(
        still.state,
        ActionState::Held,
        "still held after the restart"
    );
    decide(&client, &p, held.action.id, Decision::Approved).await;
    assert_eq!(lines(&p.work, "deploy.txt", 3).await, ["403", "200", "403"]);
    assert_eq!(sent(&seen, DEPLOY_HOST), 1);
    assert_eq!(
        state(&client, &p)
            .await
            .action(held.action.id)
            .unwrap()
            .state,
        ActionState::CarriedOut
    );
    daemon.terminate();
}

async fn log(client: &Client, project: ProjectId) -> Vec<ProjectEvent> {
    let page = client
        .request(ReadLog {
            log: LogId::Project(project),
            page: Page::OldestFirst {
                after: None,
                limit: 10_000,
            },
        })
        .await
        .unwrap();
    let LogPage::Project { events } = page else {
        panic!("a project's log")
    };
    events.into_iter().map(|e| e.event).collect()
}
