//! Each real harness against its real provider. These cost money, so they are ignored by default
//! and run only when asked for, with keys from the environment:
//!
//! ```sh
//! X100_REAL_PROVIDERS=1 ANTHROPIC_API_KEY=... OPENAI_API_KEY=... \
//!   cargo test -p x100-drivers --test real_providers -- --ignored --test-threads=1
//! ```
//!
//! Each test uses the cheapest model at low effort and makes about three model calls: the edit
//! (a tool call, then a reply) and the second turn. A model that reads before it edits makes one
//! more.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use common::*;
use x100_core::effort::Effort;
use x100_core::money::Money;
use x100_core::time::{FakeClock, Timestamp};
use x100_drivers::claude::{ClaudeCode, ClaudeCodeConfig};
use x100_drivers::codex::{Codex, CodexConfig};
use x100_drivers::{DriverOutput, Harness, TurnEnd, group_is_empty};

/// The key, if the run was asked for. Without the opt-in a test returns before any call.
fn opted_in(key_var: &str) -> Option<String> {
    if std::env::var("X100_REAL_PROVIDERS").as_deref() != Ok("1") {
        eprintln!("SKIPPED: set X100_REAL_PROVIDERS=1 to call a paid provider");
        return None;
    }
    match std::env::var(key_var) {
        Ok(key) if !key.is_empty() => Some(key),
        _ => panic!("X100_REAL_PROVIDERS=1 but {key_var} is not set"),
    }
}

const EDIT: &str = "Create a file named hello.txt in the current directory containing exactly one \
                    line: hello from x100. Then reply with the single word DONE.";
const RECALL: &str =
    "What is the name of the file you just created? Reply with the file name only.";

async fn edit_and_recall(
    harness: &dyn Harness,
    model: &str,
    env: Vec<(String, String)>,
    work: PathBuf,
) {
    let mut spec = spec(1, &work, model, Effort::Low);
    spec.env = env;
    let mut handle = harness.start(spec).await.unwrap();
    eprintln!(
        "version {} warnings {:?}",
        handle.info().version,
        handle.info().warnings
    );

    handle.turn(EDIT).await.unwrap();
    let first = until_end(&mut handle).await;
    assert_eq!(
        first.last(),
        Some(&DriverOutput::TurnEnded(TurnEnd::Completed)),
        "{first:#?}"
    );
    let written = std::fs::read_to_string(work.join("hello.txt")).unwrap();
    assert_eq!(written.trim(), "hello from x100");
    let spent: Vec<_> = first
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Spent(s) => Some(*s),
            _ => None,
        })
        .collect();
    assert_eq!(spent.len(), 1, "{first:#?}");
    assert!(
        spent[0].usage.all_input() > 0 && spent[0].usage.output > 0,
        "{spent:?}"
    );
    assert!(spent[0].cost > Money::ZERO, "{spent:?}");
    eprintln!("first turn: {spent:?}");

    handle.turn(RECALL).await.unwrap();
    let second = until_end(&mut handle).await;
    let said: String = second
        .iter()
        .filter_map(|o| match o {
            DriverOutput::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        said.contains("hello.txt"),
        "the second turn lost the first: {second:#?}"
    );

    let pgid = handle.info().pid;
    handle.stop().await;
    eventually("the process group to empty", || group_is_empty(pgid)).await;
}

#[tokio::test]
#[ignore = "calls Anthropic's paid API; see the module docs"]
async fn claude_code_with_its_real_provider() {
    let Some(key) = opted_in("ANTHROPIC_API_KEY") else {
        return;
    };
    let program = installed("claude").expect("claude must be installed for this test");
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut config = ClaudeCodeConfig::new(dir.path().join("state"));
    config.program = program;
    let harness = ClaudeCode::new(config, Arc::new(FakeClock::new(Timestamp::from_millis(1))));
    let env = vec![("ANTHROPIC_API_KEY".to_owned(), key)];
    edit_and_recall(&harness, "claude-haiku-5-5", env, work).await;
}

#[tokio::test]
#[ignore = "calls OpenAI's paid API; see the module docs"]
async fn codex_with_its_real_provider() {
    let Some(key) = opted_in("OPENAI_API_KEY") else {
        return;
    };
    let program = installed("codex").expect("codex must be installed for this test");
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut config = CodexConfig::new(dir.path().join("state"));
    config.program = program;
    let harness = Codex::new(config, Arc::new(FakeClock::new(Timestamp::from_millis(1))));
    let env = vec![("OPENAI_API_KEY".to_owned(), key)];
    edit_and_recall(&harness, "gpt-6-luna", env, work).await;
}
