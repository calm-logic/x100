//! What the sandbox adds to starting work, through the daemon as a child process: the time from a
//! builder's message to a new project's CEO until the first output of its fake driver is in the
//! transcript, with every driver in a sandbox and with the daemon's `run_without_sandbox` on.
//! Each round is a new project, so a new agent, a new sandbox and a new driver, and the two ways
//! alternate so a machine growing busier or quieter favours neither. Ignored by default:
//! `cargo test -p x100d --test measure -- --ignored --nocapture`.

mod common;

use std::time::{Duration, Instant};

use common::{Env, WAIT, create_project};
use x100_core::driver::DriverState;
use x100_core::settings::DaemonSetting;
use x100_core::transcript::Item;
use x100_proto::{ReadTranscript, SetDaemonSetting, StopDriver};

const ROUNDS: usize = 15;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, slow and only worth reading by hand"]
async fn a_brief_to_the_first_output_with_and_without_the_sandbox() {
    let env = Env::new();
    let mut daemon = env.spawn();
    let client = env.client().await;
    let (mut contained, mut uncontained) = (Vec::new(), Vec::new());
    for round in 0..ROUNDS * 2 {
        let without = round % 2 == 1;
        client
            .request(SetDaemonSetting {
                setting: DaemonSetting::RunWithoutSandbox(without),
            })
            .await
            .unwrap();
        let project = create_project(&client, &env, &format!("p{round}")).await;
        let ceo = client
            .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|p| p.ceo))
            .await
            .unwrap();
        let sent = Instant::now();
        client
            .send_chat(project, "ping", vec![], None)
            .await
            .unwrap();
        loop {
            let page = client
                .request(ReadTranscript {
                    project,
                    agent: ceo,
                    after: None,
                    limit: 100,
                })
                .await
                .unwrap();
            if page
                .entries
                .iter()
                .any(|e| matches!(e.entry.item, Item::Text { .. }))
            {
                break;
            }
            assert!(sent.elapsed() < WAIT, "no output in round {round}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let took = sent.elapsed();
        // Its warm driver would count against the machine's cap on live drivers.
        let driver = client
            .wait_for(WAIT, |s| {
                let d = s
                    .projects
                    .get(&project)?
                    .agent(ceo)?
                    .drivers
                    .last()
                    .copied()?;
                (s.projects.get(&project)?.driver(d)?.state == DriverState::Warm).then_some(d)
            })
            .await
            .unwrap();
        client
            .request(StopDriver { project, driver })
            .await
            .unwrap();
        if without {
            uncontained.push(took);
        } else {
            contained.push(took);
        }
    }
    daemon.terminate();
    let (c, u) = (median(contained.clone()), median(uncontained.clone()));
    eprintln!("in a sandbox:      median {c:?} of {contained:?}");
    eprintln!("without a sandbox: median {u:?} of {uncontained:?}");
    eprintln!("the sandbox adds {:?} at the median", c.saturating_sub(u));
}
