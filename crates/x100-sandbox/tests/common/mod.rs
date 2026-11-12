//! Fixtures for running the same tests on every backend.

#![allow(dead_code)]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use tokio::sync::OnceCell;
use x100_sandbox::{Bubblewrap, CommandLine, Container, Runtime, Sandbox, SandboxSpec};

pub const HELPER: &str = env!("CARGO_BIN_EXE_x100-sandbox");

/// Generates one test per backend for each named test function `async fn(&dyn Backend)`.
#[macro_export]
macro_rules! on_both {
    ($($name:ident),* $(,)?) => {
        mod bubblewrap {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    if $crate::common::unusable("bubblewrap", $crate::common::bubblewrap_works()) {
                        return;
                    }
                    super::$name(&$crate::common::bubblewrap()).await
                }
            )*
        }
        mod container {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    if $crate::common::unusable("containers", $crate::common::containers_work()) {
                        return;
                    }
                    super::$name($crate::common::container().await).await
                }
            )*
        }
    };
}

/// Whether a backend's tests should be passed over on this machine. A machine without the tools
/// (a Mac, a runner with no container runtime) says so and passes. With `X100_REQUIRE_SANDBOX`
/// set, as CI sets it on Linux, a missing backend is a failure, so the tests cannot go quiet.
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

pub fn bubblewrap() -> Bubblewrap {
    Bubblewrap::new(HELPER)
}

/// Docker, or Podman with `X100_CONTAINER_RUNTIME=podman`, with the base image built once for the
/// whole run.
pub async fn container() -> &'static Container {
    static CONTAINER: OnceCell<Container> = OnceCell::const_new();
    CONTAINER
        .get_or_init(|| async {
            let runtime = match std::env::var("X100_CONTAINER_RUNTIME").as_deref() {
                Ok("podman") => Runtime::podman(),
                _ => Runtime::docker(),
            };
            let c = Container::new(runtime, HELPER);
            c.ensure_image().await.expect("the base image builds");
            c
        })
        .await
}

/// A throwaway directory under the build's own, not under `/tmp`, which a sandbox replaces.
pub fn scratch() -> TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("a scratch directory")
}

/// A name no other test in this run, or a concurrent run, uses.
pub fn name(test: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "t{}-{}-{test}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

pub fn dir(path: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&path).expect("a directory");
    path
}

/// A sandbox with nothing but `worktree`, kept under `base`.
pub fn spec(test: &str, base: &Path, worktree: &Path) -> SandboxSpec {
    let name = name(test);
    SandboxSpec {
        dir: base.join(format!("sandbox-{name}")),
        name,
        worktree: worktree.to_path_buf(),
        programs: Vec::new(),
        paths: Vec::new(),
        endpoints: Vec::new(),
    }
}

pub fn sh(script: &str) -> CommandLine {
    CommandLine {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
    }
}

pub fn base_env(sandbox: &Sandbox) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("HOME".into(), sandbox.home().display().to_string()),
        ("LANG".into(), "C.UTF-8".into()),
    ]
}

/// Starts `script` inside as a driver would: through the wrapper, in a process group of its own,
/// with exactly `env`.
pub fn spawn(sandbox: &Sandbox, script: &str, env: &[(String, String)]) -> tokio::process::Child {
    let line = sandbox.wrap().apply(sh(script));
    tokio::process::Command::new(&line.program)
        .args(&line.args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(sandbox.worktree())
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("the wrapped command starts")
}

/// Runs `script` inside to the end and returns what it printed.
pub async fn run_env(sandbox: &Sandbox, script: &str, env: &[(String, String)]) -> Output {
    let child = spawn(sandbox, script, env);
    tokio::time::timeout(Duration::from_secs(90), child.wait_with_output())
        .await
        .expect("the command ends")
        .expect("the command's output")
}

pub async fn run(sandbox: &Sandbox, script: &str) -> Output {
    run_env(sandbox, script, &base_env(sandbox)).await
}

pub fn text(out: &Output) -> String {
    format!(
        "status {:?} signal {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        out.status.signal(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Waits until `path` has grown, then asserts that, once `stop` has run, it stops growing.
pub async fn assert_stops<F: std::future::Future<Output = ()>>(path: &Path, stop: F) {
    let size = || std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    grows(path).await;
    stop.await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = size();
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(size(), after, "{} still grows", path.display());
}

/// Waits until `path` exists and has something in it.
pub async fn grows(path: &Path) {
    for _ in 0..300 {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{} never grew", path.display());
}

/// The paths that hold `program` as installed on this machine, if it is on the search path.
pub fn program_paths(program: &str) -> Option<Vec<PathBuf>> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.is_file())?;
    Some(x100_sandbox::program_paths(&found))
}
