//! M12D's check through the daemon as a child process, on the fake harness and the fake clock,
//! with a stand-in on loopback for each of the three decision models' APIs.
//!
//! The daemon's proxy sends the three providers' hosts to the stand-in's HTTPS port, trusting
//! only the stand-in's test authority, and two made-up hosts to a plain server; it refuses every
//! other host. The CEO's driver sends requests with `curl` from its sandbox to the made-up hosts,
//! which no rule knows.
//!
//! - For each provider, connected with a key: an unruled write is held or let through by the
//!   stand-in's answer, and never below the write's floor; an answer under the threshold is
//!   raised a level; an error, a refusal and no answer each hold it. Each call is in the log and
//!   charged at the provider's price, and the key's use is recorded. The stand-in received the
//!   key in the provider's header, and the key is in no file of the test's directory, no event
//!   and no reply, in any common encoding. A spent budget is not asked.
//! - With none connected: an unruled read goes through, an unruled write is held, and the same
//!   driver serves both with nothing else started.

mod common;
mod standin;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use common::{DaemonProc, Env, WAIT, create_project};
use standin::{INPUT_TOKENS, Says, StandIn, plain_server};
use x100_client::Client;
use x100_core::action::{ActionKind, ActionState, AskMeAbout, ClassifiedBy, Request, Risk};
use x100_core::credential::{UseOutcome, Via};
use x100_core::decision::DecisionProvider;
use x100_core::event::ProjectEvent;
use x100_core::fold::{ActionRecord, ProjectState};
use x100_core::harness::OneOffPurpose;
use x100_core::id::{ActionId, AgentId, ProjectId};
use x100_core::ledger::ChargeSource;
use x100_core::money::{Money, Percent};
use x100_core::project::SettingChange;
use x100_core::settings::DaemonSetting;
use x100_proto::{
    ChangeProjectSetting, ErrorCode, GetDaemon, LogId, LogPage, Page, ReadLog, ReadTranscript,
    SetDaemonSetting,
};

/// Invented for this test; it opens nothing anywhere.
const KEY: &str = "dm-test-key-7f3a9c2e5b1d4a6f8e0c";
const MAIL_HOST: &str = "mail.x100.test";
const READ_HOST: &str = "read.x100.test";

const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "MAIL" }
repeat = true
steps = [
  { run = "curl -s -o /dev/null -w '%{http_code}\n' -X POST http://mail.x100.test:{{input:Port}}/send >> mail.txt" },
]

[[turn]]
when = { input_contains = "TWICE" }
repeat = true
steps = [
  { run = "for i in 1 2; do curl -s -o /dev/null -w '%{http_code}\n' -X POST http://mail.x100.test:{{input:Port}}/send >> mail.txt; done" },
]

[[turn]]
when = { input_contains = "READ" }
repeat = true
steps = [
  { run = "curl -s -o /dev/null -w '%{http_code}\n' http://read.x100.test:{{input:Port}}/page >> read.txt" },
]

[[turn]]
repeat = true
steps = [{ text = "noted" }]
"##;

struct Rig {
    env: Env,
    standin: StandIn,
    port: u16,
    seen: standin::Seen,
    daemon: DaemonProc,
    client: Client,
}

async fn rig() -> Rig {
    let env = Env::new();
    let standin = StandIn::start(env.dir.path()).await;
    let (port, seen) = plain_server().await;
    std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
    let daemon = spawn_daemon(&env, &standin);
    let client = env.client().await;
    Rig {
        env,
        standin,
        port,
        seen,
        daemon,
        client,
    }
}

/// A daemon on the test's directory with the providers' hosts sent to the stand-in. Starting it
/// again after a stop gives the restarted daemon.
fn spawn_daemon(env: &Env, standin: &StandIn) -> DaemonProc {
    let mut args = env.daemon_args(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")));
    args.extend(["--fake-clock".into(), "1794000000000".into()]);
    let mut resolve: Vec<String> = DecisionProvider::ALL
        .iter()
        .map(|p| format!("{}=127.0.0.1:{}", p.host(), standin.port))
        .collect();
    resolve.extend([MAIL_HOST, READ_HOST].map(|h| format!("{h}=127.0.0.1")));
    DaemonProc::spawn_adding(
        args,
        &[
            (x100d::PROXY_RESOLVE_ENV, resolve.join(",")),
            (
                x100d::PROXY_UPSTREAM_CA_ENV,
                standin.ca.display().to_string(),
            ),
        ],
    )
}

struct Project {
    id: ProjectId,
    ceo: AgentId,
    work: PathBuf,
}

async fn project(r: &Rig, name: &str) -> Project {
    let id = create_project(&r.client, &r.env, name).await;
    r.client
        .request(ChangeProjectSetting {
            project: id,
            change: SettingChange::AskMeAbout(AskMeAbout::HighRiskOnly),
        })
        .await
        .unwrap();
    let ceo = r.client.project(id).unwrap().project().unwrap().ceo;
    let work = x100_store::Git::new(PathBuf::from(r.env.project_dir(name))).worktree_path(ceo);
    Project { id, ceo, work }
}

fn egress_to<'a>(s: &'a ProjectState, host: &'a str) -> impl Iterator<Item = &'a ActionRecord> {
    s.actions()
        .filter(move |a| matches!(&a.action.request, Request::Egress { host: h, .. } if h == host))
}

fn known(r: &Rig, p: &Project, host: &str) -> HashSet<ActionId> {
    let s = r.client.project(p.id).unwrap();
    egress_to(&s, host).map(|a| a.action.id).collect()
}

/// Sends `body` to the CEO and waits for the new requests to `host` it leads to, `n` of them,
/// each ruled on and, if let through, ended.
async fn act(r: &Rig, p: &Project, body: &str, host: &str, n: usize) -> Vec<ActionRecord> {
    let before = known(r, p, host);
    let message = format!("{body}\nPort: {}", r.port);
    r.client
        .send_chat(p.id, &message, vec![], None)
        .await
        .unwrap();
    let id = p.id;
    let host = host.to_owned();
    r.client
        .wait_for(WAIT, move |s| {
            let st = s.projects.get(&id)?;
            let new: Vec<ActionRecord> = egress_to(st, &host)
                .filter(|a| !before.contains(&a.action.id))
                .cloned()
                .collect();
            let done = |a: &ActionRecord| {
                matches!(
                    a.state,
                    ActionState::Held | ActionState::CarriedOut | ActionState::Failed
                )
            };
            (new.len() == n && new.iter().all(done)).then_some(new)
        })
        .await
        .expect("the requests are ruled on")
}

async fn one(r: &Rig, p: &Project, body: &str) -> ActionRecord {
    act(r, p, body, MAIL_HOST, 1).await.remove(0)
}

async fn events(client: &Client, log: LogId) -> Vec<serde_json::Value> {
    let page = client
        .request(ReadLog {
            log,
            page: Page::OldestFirst {
                after: None,
                limit: 10_000,
            },
        })
        .await
        .unwrap();
    match page {
        LogPage::Project { events } => events
            .into_iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect(),
        LogPage::Daemon { events } => events
            .into_iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect(),
    }
}

async fn project_events(client: &Client, id: ProjectId) -> Vec<ProjectEvent> {
    events(client, LogId::Project(id))
        .await
        .into_iter()
        .map(|v| serde_json::from_value::<x100_core::event::Envelope<ProjectEvent>>(v).unwrap())
        .map(|e| e.event)
        .collect()
}

/// The decision model's call behind a classification, and what it was charged.
async fn call_of(r: &Rig, p: &Project, a: &ActionRecord) -> (DecisionProvider, Option<Money>) {
    let ClassifiedBy::Classifier(call) = a.classification.clone().unwrap().by else {
        panic!("not classified by a call: {a:?}")
    };
    let events = project_events(&r.client, p.id).await;
    let made = events
        .iter()
        .find_map(|e| match e {
            ProjectEvent::OneOffCalled(c) if c.id == call => Some(c.clone()),
            _ => None,
        })
        .expect("the call is in the log");
    assert_eq!(
        made.purpose,
        OneOffPurpose::Classify {
            action: a.action.id
        }
    );
    let charged = events.iter().find_map(|e| match e {
        ProjectEvent::Charged(c) if c.source == ChargeSource::OneOff { call } => {
            assert_eq!(c.usage.input, INPUT_TOKENS);
            assert_eq!(c.usage.output, 0);
            Some(c.cost)
        }
        _ => None,
    });
    (made.provider, charged)
}

/// The lines a step wrote to `file` in the worktree, once it has `n` of them: the gate rules
/// before `curl` has written what it got.
async fn lines(dir: &Path, file: &str, n: usize) -> Vec<String> {
    let until = std::time::Instant::now() + WAIT;
    loop {
        let text = std::fs::read_to_string(dir.join(file)).unwrap_or_default();
        let got: Vec<String> = text.lines().map(str::to_owned).collect();
        if got.len() >= n {
            return got;
        }
        assert!(std::time::Instant::now() < until, "{file} has {got:?}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_decision_model_classifies_what_no_rule_covers() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let mut r = rig().await;
    for (provider, price) in [
        (DecisionProvider::Gemini, Money::from_micros(200)),
        (DecisionProvider::OpenAi, Money::from_micros(200)),
        (DecisionProvider::Jev, Money::from_micros(84)),
    ] {
        let name = format!("{provider:?}").to_lowercase();
        let credential = r
            .client
            .connect_decision_model(provider, KEY)
            .await
            .expect("connected");
        r.client
            .wait_for(WAIT, |s| {
                let m = s.daemon.settings().decision_model?;
                (m.provider == provider && m.credential == credential).then_some(())
            })
            .await
            .unwrap();
        let p = project(&r, &name).await;
        let from = r.standin.received().len();

        // Sure and Low: the write's floor makes it Medium, which goes through at High risk only.
        r.standin.say(Says::Labels {
            kind: "sends_to_people_outside",
            risk: "low",
            confidence: 0.95,
        });
        let a = one(&r, &p, "MAIL").await;
        let c = a.classification.clone().unwrap();
        assert_eq!(a.state, ActionState::CarriedOut, "{name}: {}", c.why);
        assert_eq!((c.kind, c.risk), (ActionKind::SendOutside, Risk::Medium));
        assert!(c.why.contains("go below Medium"), "{}", c.why);
        assert_eq!(call_of(&r, &p, &a).await, (provider, Some(price)));

        // Sure and High: held, and nothing reaches the host.
        r.standin.say(Says::Labels {
            kind: "deletes_data",
            risk: "high",
            confidence: 0.9,
        });
        let a = one(&r, &p, "MAIL").await;
        assert_eq!(a.state, ActionState::Held, "{name}");
        assert_eq!(
            a.classification.clone().unwrap().kind,
            ActionKind::DeleteData
        );

        // Medium but unsure: raised to High, so held.
        r.standin.say(Says::Labels {
            kind: "none_of_these",
            risk: "medium",
            confidence: 0.5,
        });
        let a = one(&r, &p, "MAIL").await;
        let c = a.classification.clone().unwrap();
        assert_eq!((a.state, c.risk), (ActionState::Held, Risk::High), "{name}");
        assert!(c.why.contains("raised to High"), "{}", c.why);
        // The same, sure, goes through.
        r.standin.say(Says::Labels {
            kind: "none_of_these",
            risk: "medium",
            confidence: 0.8,
        });
        assert_eq!(one(&r, &p, "MAIL").await.state, ActionState::CarriedOut);

        // An error, a refusal and no answer each hold it. The error quotes the key, which the
        // builder sees only as its reference.
        for (says, said) in [
            (Says::Fail(401), "answered 401"),
            (Says::Refuse, ""),
            (Says::HangUp, ""),
        ] {
            r.standin.say(says.clone());
            let a = one(&r, &p, "MAIL").await;
            let c = a.classification.clone().unwrap();
            assert_eq!(
                (a.state, c.risk),
                (ActionState::Held, Risk::High),
                "{says:?}"
            );
            assert!(c.why.contains("gave no answer"), "{}", c.why);
            assert!(c.why.contains(said), "{}", c.why);
            assert!(!c.why.contains(KEY), "{}", c.why);
            if matches!(says, Says::Fail(_)) {
                assert!(c.why.contains(&credential.to_string()), "{}", c.why);
            }
            let (_, charged) = call_of(&r, &p, &a).await;
            // Only an answer that counts its input is charged.
            assert_eq!(charged.is_some(), matches!(says, Says::Refuse), "{says:?}");
        }

        // Every request carried the key in the provider's header, to the provider's host.
        let sent = &r.standin.received()[from..];
        assert_eq!(sent.len(), 7);
        for req in sent {
            assert_eq!(req.host, provider.host());
            assert!(req.path.ends_with("decisions") || req.path == "/v1/systemone");
            // The action is the input; the key is in the header alone.
            let body = req.body.to_string();
            assert!(body.contains(MAIL_HOST) && !body.contains(KEY), "{body}");
            let header = match provider {
                DecisionProvider::Gemini => req.headers.get("x-goog-api-key").cloned(),
                _ => req.headers.get("authorization").cloned(),
            };
            let want = match provider {
                DecisionProvider::Gemini => KEY.to_owned(),
                _ => format!("Bearer {KEY}"),
            };
            assert_eq!(header.as_deref(), Some(want.as_str()));
        }
        // Each use of the key is recorded, for the agent whose action it was.
        let uses: Vec<_> = project_events(&r.client, p.id)
            .await
            .into_iter()
            .filter_map(|e| match e {
                ProjectEvent::CredentialUsed(u) => Some(u),
                _ => None,
            })
            .collect();
        assert_eq!(uses.len(), 7);
        assert!(uses.iter().all(|u| u.credential == credential
            && u.agent == p.ceo
            && u.outcome == UseOutcome::Injected
            && u.via
                == Via::Proxy {
                    host: provider.host().into()
                }));
        assert_eq!(
            lines(&p.work, "mail.txt", 7).await,
            ["200", "403", "403", "200", "403", "403", "403"]
        );
    }
    assert_eq!(r.seen.lock().unwrap().get(MAIL_HOST).copied(), Some(6));

    // A spent budget is not asked: the first call of the turn passes the project's budget, so
    // the second request of the same turn is held without a call.
    r.client
        .connect_decision_model(DecisionProvider::Gemini, KEY)
        .await
        .unwrap();
    let p = project(&r, "frugal").await;
    r.client
        .request(ChangeProjectSetting {
            project: p.id,
            change: SettingChange::Budget(Money::from_micros(100)),
        })
        .await
        .unwrap();
    r.standin.say(Says::Labels {
        kind: "none_of_these",
        risk: "low",
        confidence: 0.95,
    });
    let asked = r.standin.received().len();
    let both = act(&r, &p, "TWICE", MAIL_HOST, 2).await;
    assert_eq!(r.standin.received().len(), asked + 1, "asked once");
    let unasked = both
        .iter()
        .find(|a| a.state == ActionState::Held)
        .expect("one is held");
    let c = unasked.classification.clone().unwrap();
    assert_eq!(c.by, ClassifiedBy::Rules);
    assert!(c.why.contains("budget is spent"), "{}", c.why);

    no_key_anywhere(&r).await;
    r.daemon.terminate();
}

/// The key, raw and in the encodings a careless program might use, is in no file under the
/// test's directory (the daemon's data, the projects, the worktrees), no event of either log, and
/// no reply the client got.
async fn no_key_anywhere(r: &Rig) {
    let b64 = |e: &base64::engine::GeneralPurpose| e.encode(KEY.as_bytes());
    let hex: String = KEY.bytes().map(|b| format!("{b:02x}")).collect();
    let forms = [
        KEY.to_owned(),
        hex,
        b64(&base64::engine::general_purpose::STANDARD),
        b64(&base64::engine::general_purpose::URL_SAFE),
    ];
    let check = |what: &str, haystack: &[u8]| {
        for form in &forms {
            assert!(
                !haystack.windows(form.len()).any(|w| w == form.as_bytes()),
                "the key is in {what}"
            );
        }
    };
    for file in files(r.env.dir.path()) {
        if let Ok(bytes) = std::fs::read(&file) {
            check(&file.display().to_string(), &bytes);
        }
    }
    let daemon = r.client.request(GetDaemon {}).await.unwrap();
    check("the daemon's state", &serde_json::to_vec(&daemon).unwrap());
    check(
        "the daemon's log",
        &serde_json::to_vec(&events(&r.client, LogId::Daemon).await).unwrap(),
    );
    for (id, _) in daemon.projects() {
        check(
            "a project's log",
            &serde_json::to_vec(&events(&r.client, LogId::Project(id)).await).unwrap(),
        );
        let state = r.client.project(id).unwrap();
        check("a project's state", &serde_json::to_vec(&state).unwrap());
        let ceo = state.project().unwrap().ceo;
        let transcript = r
            .client
            .request(ReadTranscript {
                project: id,
                agent: ceo,
                after: None,
                limit: 10_000,
            })
            .await
            .unwrap();
        check("a transcript", &serde_json::to_vec(&transcript).unwrap());
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(e.path());
            } else if kind.is_file() {
                out.push(e.path());
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_decision_model_a_read_goes_and_a_write_waits_and_nothing_is_started() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let mut r = rig().await;
    let p = project(&r, "unjudged").await;
    let read = act(&r, &p, "READ", READ_HOST, 1).await.remove(0);
    let c = read.classification.clone().unwrap();
    assert_eq!(read.state, ActionState::CarriedOut);
    assert_eq!((c.risk, c.by), (Risk::Low, ClassifiedBy::Rules));
    assert!(c.why.contains("classifier.not_connected.read"), "{}", c.why);
    assert_eq!(lines(&p.work, "read.txt", 1).await, ["200"]);

    let drivers = |s: &ProjectState| -> Vec<_> { s.drivers().map(|d| d.driver.id).collect() };
    let before = drivers(&r.client.project(p.id).unwrap());
    let write = one(&r, &p, "MAIL").await;
    let c = write.classification.clone().unwrap();
    assert_eq!(write.state, ActionState::Held);
    assert_eq!((c.risk, c.by), (Risk::High, ClassifiedBy::Rules));
    assert!(
        c.why.contains("classifier.not_connected.write"),
        "{}",
        c.why
    );
    assert_eq!(lines(&p.work, "mail.txt", 1).await, ["403"]);
    assert_eq!(r.seen.lock().unwrap().get(MAIL_HOST), None);

    // Nothing was asked and nothing was started for it: no call, no request to any provider,
    // and the driver that sent the read sent the write.
    assert!(r.standin.received().is_empty());
    let events = project_events(&r.client, p.id).await;
    let asked = |e: &ProjectEvent| match e {
        ProjectEvent::OneOffCalled(_) => true,
        ProjectEvent::Charged(c) => matches!(c.source, ChargeSource::OneOff { .. }),
        _ => false,
    };
    assert!(!events.iter().any(asked));
    let after = drivers(&r.client.project(p.id).unwrap());
    assert_eq!(before, after, "no driver started");
    let Request::Egress { driver: wrote, .. } = write.action.request else {
        unreachable!()
    };
    let Request::Egress {
        driver: read_by, ..
    } = read.action.request
    else {
        unreachable!()
    };
    assert_eq!(wrote, read_by);
    r.daemon.terminate();
}

async fn set_threshold(client: &Client, percent: u8) -> Result<(), x100_client::ClientError> {
    client
        .request(SetDaemonSetting {
            setting: DaemonSetting::DecisionThreshold(Percent::new(percent).unwrap()),
        })
        .await
        .map(drop)
}

async fn threshold(client: &Client) -> u8 {
    let daemon = client.request(GetDaemon {}).await.unwrap();
    daemon.settings().decision_threshold.value()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_threshold_decides_which_answers_are_raised_and_survives_a_restart() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let mut r = rig().await;
    r.client
        .connect_decision_model(DecisionProvider::Gemini, KEY)
        .await
        .unwrap();
    let p = project(&r, "threshold").await;
    assert_eq!(threshold(&r.client).await, 70);

    // Medium and 80 percent sure: sure enough under the default.
    let medium = Says::Labels {
        kind: "none_of_these",
        risk: "medium",
        confidence: 0.8,
    };
    r.standin.say(medium.clone());
    let a = one(&r, &p, "MAIL").await;
    assert_eq!(a.state, ActionState::CarriedOut);
    assert_eq!(a.classification.unwrap().risk, Risk::Medium);

    // Out of range is refused and changes nothing.
    for bad in [49, 96, 0, 100] {
        match set_threshold(&r.client, bad).await {
            Err(x100_client::ClientError::Rpc(e)) => {
                assert_eq!(e.code, ErrorCode::Rejected, "{bad}")
            }
            other => panic!("{bad}: {other:?}"),
        }
    }
    assert_eq!(threshold(&r.client).await, 70);

    // The same answer is not sure enough under 90.
    set_threshold(&r.client, 90).await.unwrap();
    let raised = |a: &ActionRecord| {
        let c = a.classification.clone().unwrap();
        (a.state, c.risk, c.why.contains("raised to High"))
    };
    let a = one(&r, &p, "MAIL").await;
    assert_eq!(raised(&a), (ActionState::Held, Risk::High, true));

    // The setting is on disk, not only in the running daemon, and still rules after a restart.
    r.daemon.terminate();
    r.daemon = spawn_daemon(&r.env, &r.standin);
    r.client = r.env.client().await;
    assert_eq!(threshold(&r.client).await, 90);
    let p = project(&r, "after-restart").await;
    r.standin.say(medium);
    let a = one(&r, &p, "MAIL").await;
    assert_eq!(raised(&a), (ActionState::Held, Risk::High, true));
    r.daemon.terminate();
}
