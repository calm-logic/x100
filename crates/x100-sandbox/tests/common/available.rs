//! Whether this machine can make a sandbox, decided once per test process. It is included by path
//! into the test fixtures of every package whose tests start a daemon, so that one rule decides
//! for all of them and no crate exists only to hold it.
//!
//! A machine without the tools (a Mac, a runner with no container runtime) passes over the tests
//! that need a sandbox and runs the rest without one. With `X100_REQUIRE_SANDBOX` set, as CI sets
//! it on Linux, a missing sandbox is a failure, so nothing can go quiet and nothing runs
//! uncontained.

#![allow(dead_code)]

use std::sync::OnceLock;

/// Whether a backend's tests should be passed over on this machine. With `X100_REQUIRE_SANDBOX`
/// set a missing backend is a failure.
pub fn unusable(backend: &str, works: bool) -> bool {
    if works {
        return false;
    }
    assert!(
        std::env::var_os("X100_REQUIRE_SANDBOX").is_none(),
        "{backend} is required here and does not work"
    );
    eprintln!("SKIPPED: {backend} does not work on this machine");
    true
}

fn succeeds(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The bubblewrap backend needs Linux, `bwrap`, and user namespaces an ordinary user may make.
pub fn bubblewrap_works() -> bool {
    cfg!(target_os = "linux")
        && succeeds("bwrap", &["--version"])
        && succeeds("unshare", &["--user", "--map-root-user", "true"])
}

/// The container tests run host programs inside the container, so they need Linux as well as
/// a runtime that answers.
pub fn containers_work() -> bool {
    let runtime = match std::env::var("X100_CONTAINER_RUNTIME").as_deref() {
        Ok("podman") => "podman",
        _ => "docker",
    };
    cfg!(target_os = "linux") && succeeds(runtime, &["info"])
}

/// Whether the backend a daemon started by a test would choose can make sandboxes here: the one
/// `X100_SANDBOX` names, else bubblewrap on Linux and Docker elsewhere, as the daemon chooses.
pub fn sandbox_works() -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| match std::env::var("X100_SANDBOX").as_deref() {
        Ok("docker") | Ok("podman") => containers_work(),
        Ok(_) => bubblewrap_works(),
        Err(_) if cfg!(target_os = "linux") => bubblewrap_works(),
        Err(_) => containers_work(),
    })
}

/// For a test whose assertions mean something only inside a sandbox: true when it should return
/// at once, having said why.
pub fn skip_without_sandbox() -> bool {
    if sandbox_works() {
        return false;
    }
    assert!(
        std::env::var_os("X100_REQUIRE_SANDBOX").is_none(),
        "a sandbox is required here and none can be made"
    );
    eprintln!("SKIPPED: no sandbox can be made on this machine");
    true
}
