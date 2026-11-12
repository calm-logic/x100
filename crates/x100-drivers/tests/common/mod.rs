//! Helpers shared by the integration tests.

#![allow(dead_code)]

pub mod fake;
pub mod terminal;

use std::path::{Path, PathBuf};
use std::time::Duration;

use x100_core::effort::Effort;
use x100_core::harness::ModelName;
use x100_core::id::DriverId;
use x100_drivers::{
    CommandWrapper, DriverHandle, DriverOutput, DriverSpec, Expose, Permissions, SpawnHook,
    StallPolicy, ToolEndpoint,
};

/// How long a test waits for a driver to say something before it gives up. Only a guard against a
/// hung test: no test waits this long for anything it expects.
pub const PATIENCE: Duration = Duration::from_secs(120);

pub fn stall_policy() -> StallPolicy {
    StallPolicy {
        silent: Duration::from_secs(5 * 60),
        while_tool_runs: Duration::from_secs(30 * 60),
        // Stalls are checked by hand after the fake clock moves.
        check_every: Duration::from_secs(3600),
    }
}

pub fn spec(n: u128, dir: &Path, model: &str, effort: Effort) -> DriverSpec {
    DriverSpec {
        driver: DriverId::from_u128(n),
        model: ModelName(model.to_owned()),
        effort,
        dir: dir.to_owned(),
        // Beside the worktree, in the test's own directory.
        state: dir.with_file_name(format!("driver-{n}")),
        wrap: CommandWrapper::identity(),
        expose: Expose::nothing(),
        tmp: None,
        start_pack: "You are a test agent.".into(),
        tools: None,
        env: Vec::new(),
        stall: stall_policy(),
        permissions: Permissions::AllowAll,
        on_spawn: SpawnHook::ignore(),
        login: None,
    }
}

pub fn endpoint(url: &str) -> ToolEndpoint {
    ToolEndpoint {
        url: url.to_owned(),
        token: "drv-token-123".into(),
    }
}

pub async fn next(handle: &mut DriverHandle) -> Option<DriverOutput> {
    tokio::time::timeout(PATIENCE, handle.next())
        .await
        .expect("the driver said nothing for too long")
}

/// Everything up to and including the end of a turn or compaction, or a failure.
pub async fn until_end(handle: &mut DriverHandle) -> Vec<DriverOutput> {
    let mut seen = Vec::new();
    while let Some(out) = next(handle).await {
        let end = matches!(
            out,
            DriverOutput::TurnEnded(_) | DriverOutput::Compacted | DriverOutput::Failed(_)
        );
        seen.push(out);
        if end {
            return seen;
        }
    }
    seen
}

/// Reads until `wanted` matches, returning what came before it and the match.
pub async fn until(
    handle: &mut DriverHandle,
    wanted: impl Fn(&DriverOutput) -> bool,
) -> (Vec<DriverOutput>, DriverOutput) {
    let mut seen = Vec::new();
    loop {
        let out = next(handle)
            .await
            .unwrap_or_else(|| panic!("the stream ended before the output wanted; saw {seen:#?}"));
        if wanted(&out) {
            return (seen, out);
        }
        seen.push(out);
    }
}

/// Polls real time for a condition about processes or files, which no clock drives.
pub async fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn process_alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

pub fn read_pid(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// A program on PATH, or `None` with a note on stderr so a skipped test is visible.
pub fn installed(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file());
    if found.is_none() {
        eprintln!("SKIPPED: {program} is not installed on this machine");
    }
    found
}
