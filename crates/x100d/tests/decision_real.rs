//! One real call to each decision model, on the builder's own key. Ignored: each costs money and
//! leaves the machine. Run one with
//!
//! ```text
//! X100_REAL_DECISIONS=1 X100_REAL_DECISIONS_DIR=<dir> \
//!   cargo test -p x100d --test decision_real -- --ignored gemini
//! ```
//!
//! `<dir>` holds `gemini`, `openai` and `jev`: each a data directory made for this, in which the
//! builder connected that provider's key in Settings, with the daemon started with
//! `X100_NO_KEYCHAIN=1` as these tests start it. The test never sees the key: the daemon's vault
//! holds it and its proxy puts it in the request. On the fake harness, the CEO reads a page from a
//! host no rule knows, so the gate asks the connected model.

mod common;

use std::path::{Path, PathBuf};

use common::{DaemonProc, Env, WAIT, create_project};
use x100_core::action::{ActionState, ClassifiedBy, Request};
use x100_core::decision::DecisionProvider;
use x100_core::event::ProjectEvent;
use x100_core::ledger::ChargeSource;
use x100_proto::{LogId, LogPage, Page, ReadLog};

const HOST: &str = "example.com";

const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "READ" }
repeat = true
steps = [{ run = "curl -s -o /dev/null -w '%{http_code}\n' http://example.com/ >> read.txt" }]

[[turn]]
repeat = true
steps = [{ text = "noted" }]
"##;

/// The data directory the builder prepared for `name`, if the run was asked for.
fn prepared(name: &str) -> Option<PathBuf> {
    if std::env::var_os("X100_REAL_DECISIONS").is_none() {
        eprintln!("SKIPPED: set X100_REAL_DECISIONS=1 to make a real call");
        return None;
    }
    let dir = std::env::var_os("X100_REAL_DECISIONS_DIR")
        .map(PathBuf::from)
        .expect("X100_REAL_DECISIONS_DIR names the prepared data directories")
        .join(name);
    assert!(dir.is_dir(), "{} was not prepared", dir.display());
    Some(dir)
}

async fn one_real_call(provider: DecisionProvider, name: &str) {
    let Some(data) = prepared(name) else { return };
    if common::available::skip_without_sandbox() {
        return;
    }
    let env = Env::new();
    std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
    let args = vec![
        "--data-dir".into(),
        data.clone().into_os_string(),
        "--projects-root".into(),
        env.projects_root().into_os_string(),
        "--scripts-dir".into(),
        env.scripts.clone().into_os_string(),
        "--fake-harness".into(),
        Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")).into(),
    ];
    let mut daemon = DaemonProc::spawn(args);
    let mut options = x100_client::Options::new(data.join("x100d.sock"), "test");
    options.startup_timeout = WAIT;
    let client = x100_client::Client::connect(options).await.unwrap();
    let connected = client
        .with_state(|s| s.daemon.settings().decision_model)
        .expect("a decision model is connected in the prepared directory");
    assert_eq!(connected.provider, provider, "the wrong one is connected");

    let project = create_project(&client, &env, &format!("real-{name}")).await;
    client
        .send_chat(project, "READ", vec![], None)
        .await
        .unwrap();
    let action = client
        .wait_for(WAIT, |s| {
            s.projects
                .get(&project)?
                .actions()
                .find(|a| {
                    matches!(&a.action.request, Request::Egress { host, .. } if host == HOST)
                        && !matches!(a.state, ActionState::Proposed | ActionState::Classified)
                })
                .cloned()
        })
        .await
        .expect("the read is ruled on");
    let c = action.classification.unwrap();
    eprintln!("{name}: {:?} at {:?}: {}", c.kind, c.risk, c.why);
    let ClassifiedBy::Classifier(call) = c.by else {
        panic!("the model gave no answer: {}", c.why)
    };
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
    let charge = events
        .iter()
        .find_map(|e| match &e.event {
            ProjectEvent::Charged(ch) if ch.source == ChargeSource::OneOff { call } => Some(ch),
            _ => None,
        })
        .expect("the call is charged");
    assert!(charge.usage.input > 0);
    eprintln!(
        "{name}: {} input tokens, {} millionths of a dollar",
        charge.usage.input,
        charge.cost.micros()
    );
    daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a real call to Gemini on the builder's key"]
async fn gemini() {
    one_real_call(DecisionProvider::Gemini, "gemini").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a real call to OpenAI on the builder's key"]
async fn openai() {
    one_real_call(DecisionProvider::OpenAi, "openai").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a real call to Jev on the builder's key"]
async fn jev() {
    one_real_call(DecisionProvider::Jev, "jev").await;
}
