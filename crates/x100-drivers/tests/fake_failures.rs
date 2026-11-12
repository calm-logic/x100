//! The scripted fake harness failing on cue: crashing, stalling on the fake clock, provider
//! errors, and being stopped.

mod common;

use std::time::Duration;

use common::fake::*;
use common::*;
use x100_core::driver::Failure;
use x100_core::harness::ProviderFailure;
use x100_drivers::{DriverOutput, Harness, InputError, group_is_empty};

#[tokio::test]
async fn a_fake_told_to_crash_is_reported_failed_with_the_reason() {
    let s = setup(
        r#"[[turn]]
steps = [{ text = "about to crash" }, { crash = 3 }]"#,
    );
    let mut handle = s.harness.start(spec_in(&s.work)).await.unwrap();
    let pid = handle.info().pid;
    handle.turn("go").await.unwrap();
    let seen = until_end(&mut handle).await;
    assert_eq!(
        seen.first(),
        Some(&DriverOutput::Text("about to crash".into()))
    );
    assert_eq!(
        seen.last(),
        Some(&DriverOutput::Failed(Failure::ProcessExited {
            code: Some(3)
        }))
    );
    assert_eq!(next(&mut handle).await, None);
    assert!(group_is_empty(pid));
}

#[tokio::test]
async fn a_fake_told_to_stall_fails_once_the_timeout_passes_on_the_fake_clock_and_not_before() {
    let s = setup(
        r#"[[turn]]
steps = [{ text = "working" }, { stall = true }]"#,
    );
    let mut handle = s.harness.start(spec_in(&s.work)).await.unwrap();
    let control = handle.control();
    handle.turn("go").await.unwrap();
    assert_eq!(
        next(&mut handle).await,
        Some(DriverOutput::Text("working".into()))
    );

    s.clock.advance(Duration::from_secs(5 * 60));
    assert!(
        !control.check_stall().await,
        "silent for exactly the timeout is not past it"
    );
    s.clock.advance(Duration::from_millis(1));
    assert!(control.check_stall().await);
    assert_eq!(
        next(&mut handle).await,
        Some(DriverOutput::Failed(Failure::Stalled {
            silent_for: Duration::from_millis(5 * 60 * 1000 + 1)
        }))
    );
    assert_eq!(next(&mut handle).await, None);
}

#[tokio::test]
async fn the_stall_timeout_is_longer_while_a_tool_runs_and_does_not_run_between_turns() {
    let s = setup(
        r#"[[turn]]
steps = [{ text = "idle next" }]
[[turn]]
steps = [{ run = "sleep 600" }]"#,
    );
    let mut handle = s.harness.start(spec_in(&s.work)).await.unwrap();
    let control = handle.control();
    handle.turn("one").await.unwrap();
    until_end(&mut handle).await;
    // A warm driver owes nothing, however long it waits.
    s.clock.advance(Duration::from_secs(24 * 3600));
    assert!(!control.check_stall().await);

    handle.turn("two").await.unwrap();
    let (_, call) = until(&mut handle, |o| matches!(o, DriverOutput::ToolCall { .. })).await;
    assert!(matches!(call, DriverOutput::ToolCall { name, .. } if name == "run"));
    s.clock.advance(Duration::from_secs(29 * 60));
    assert!(
        !control.check_stall().await,
        "a running command has thirty minutes"
    );
    s.clock
        .advance(Duration::from_secs(60) + Duration::from_millis(1));
    assert!(control.check_stall().await);
    assert!(matches!(
        next(&mut handle).await,
        Some(DriverOutput::Failed(Failure::Stalled { .. }))
    ));
}

#[tokio::test]
async fn a_provider_error_in_the_script_is_reported_as_that_provider_failure() {
    let cases = [
        (
            "rate_limited",
            Some(ProviderFailure::RateLimited { retry_after: None }),
        ),
        ("key_refused", Some(ProviderFailure::KeyRefused)),
        (
            "outage",
            Some(ProviderFailure::Outage {
                detail: "boom".into(),
            }),
        ),
        ("run", None),
    ];
    for (kind, provider) in cases {
        let s = setup(&format!(
            "[[turn]]\nsteps = [{{ fail = \"{kind}\", detail = \"boom\" }}]"
        ));
        let mut handle = s.harness.start(spec_in(&s.work)).await.unwrap();
        handle.turn("go").await.unwrap();
        assert_eq!(
            until_end(&mut handle).await,
            [DriverOutput::Failed(Failure::Error {
                detail: "boom".into(),
                provider
            })]
        );
    }
}

#[tokio::test]
async fn stopping_a_driver_stops_its_child_and_everything_in_its_process_group() {
    let s = setup(
        r#"[[turn]]
steps = [{ run = "echo $$ > pid.txt; exec sleep 300" }]"#,
    );
    let mut handle = s.harness.start(spec_in(&s.work)).await.unwrap();
    let pgid = handle.info().pid;
    handle.turn("go").await.unwrap();
    until(&mut handle, |o| matches!(o, DriverOutput::ToolCall { .. })).await;
    let pid_file = s.work.join("pid.txt");
    eventually("the command to start", || read_pid(&pid_file).is_some()).await;
    let sleeper = read_pid(&pid_file).unwrap();
    assert!(process_alive(sleeper));

    handle.stop().await;
    let rest = until_end(&mut handle).await;
    assert!(
        rest.iter().all(|o| !matches!(o, DriverOutput::Failed(_))),
        "{rest:?}"
    );
    eventually("the process group to empty", || group_is_empty(pgid)).await;
    eventually("the command to die", || !process_alive(sleeper)).await;
    assert_eq!(handle.turn("more").await, Err(InputError::Ended));
}
