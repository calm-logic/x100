//! The measurements M10 records, in `MEASUREMENTS.md`. Ignored by default: they are slow, and
//! the numbers only mean something on a quiet machine.
//!
//! `X100_MEASURE=bubblewrap|container` picks the backend (both by default).
//! `cargo test -p x100-sandbox --test measure -- --ignored --nocapture --test-threads 1`

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use x100_sandbox::Backend;

async fn backends() -> Vec<(&'static str, Box<dyn Backend>)> {
    let which = std::env::var("X100_MEASURE").unwrap_or_default();
    let mut all: Vec<(&'static str, Box<dyn Backend>)> = Vec::new();
    if which.is_empty() || which == "bubblewrap" {
        all.push(("bubblewrap", Box::new(bubblewrap())));
    }
    if which.is_empty() || which == "container" {
        all.push(("container", Box::new(container().await.clone())));
    }
    all
}

fn median(mut xs: Vec<Duration>) -> Duration {
    xs.sort();
    xs[xs.len() / 2]
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, run by hand"]
async fn create_and_start() {
    for (label, backend) in backends().await {
        let base = scratch();
        let work = dir(base.path().join("work"));
        let (mut creates, mut destroys) = (Vec::new(), Vec::new());
        for _ in 0..10 {
            let t = Instant::now();
            let sandbox = backend
                .create(spec("measure", base.path(), &work))
                .await
                .unwrap();
            creates.push(t.elapsed());
            let t = Instant::now();
            sandbox.destroy().await.unwrap();
            destroys.push(t.elapsed());
        }
        let sandbox = backend
            .create(spec("measure", base.path(), &work))
            .await
            .unwrap();
        let env = base_env(&sandbox);
        let (mut inside, mut outside) = (Vec::new(), Vec::new());
        for _ in 0..30 {
            let t = Instant::now();
            let out = run_env(&sandbox, "true", &env).await;
            assert!(out.status.success(), "{}", text(&out));
            inside.push(t.elapsed());
            let t = Instant::now();
            let out = tokio::process::Command::new("/bin/sh")
                .args(["-c", "true"])
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k, v)))
                .output()
                .await
                .unwrap();
            assert!(out.status.success());
            outside.push(t.elapsed());
        }
        sandbox.destroy().await.unwrap();
        println!(
            "{label}: create {} (median of 10), destroy {}, `sh -c true` inside {} vs outside {} (median of 30)",
            ms(median(creates)),
            ms(median(destroys)),
            ms(median(inside)),
            ms(median(outside)),
        );
    }
}

/// A clean build of the workspace without the desktop app, from a warm dependency cache, once
/// outside any sandbox and once inside each backend.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, run by hand: several clean builds"]
async fn workspace_build() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let cargo_home = std::env::var("CARGO_HOME").map_or(home.join(".cargo"), PathBuf::from);
    let rustup_home = std::env::var("RUSTUP_HOME").map_or(home.join(".rustup"), PathBuf::from);
    let base = scratch();
    let tree = dir(base.path().join("x100"));
    // The committed workspace, not the working files other agents are changing.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let archive = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "git -C {} archive HEAD | tar -x -C {}",
            repo.display(),
            tree.display()
        ))
        .status()
        .unwrap();
    assert!(archive.success());
    let env = |target: &str, home_dir: &Path| -> Vec<(String, String)> {
        vec![
            (
                "PATH".into(),
                format!("{}/bin:/usr/bin:/bin", cargo_home.display()),
            ),
            ("HOME".into(), home_dir.display().to_string()),
            ("CARGO_HOME".into(), cargo_home.display().to_string()),
            ("RUSTUP_HOME".into(), rustup_home.display().to_string()),
            (
                "CARGO_TARGET_DIR".into(),
                tree.join(target).display().to_string(),
            ),
            ("CARGO_TERM_COLOR".into(), "never".into()),
        ]
    };
    let build = "cargo build --offline --workspace --exclude x100-app 2>&1 | grep -c Compiling";
    let rounds: usize = std::env::var("X100_MEASURE_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(2);
    let backends = backends().await;
    // `None` builds outside any sandbox. Every other round runs in the opposite order, so a
    // machine that gets busier or quieter during the run favours no side.
    let places: Vec<Option<usize>> = std::iter::once(None)
        .chain((0..backends.len()).map(Some))
        .collect();
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); places.len()];
    for round in 0..rounds {
        let mut order: Vec<usize> = (0..places.len()).collect();
        if round % 2 == 1 {
            order.reverse();
        }
        for i in order {
            let target = format!("target-{round}-{i}");
            let (secs, compiled) = match places[i] {
                None => {
                    let t = Instant::now();
                    let out = tokio::process::Command::new("sh")
                        .args(["-c", build])
                        .env_clear()
                        .envs(env(&target, &home))
                        .current_dir(&tree)
                        .output()
                        .await
                        .unwrap();
                    (t.elapsed().as_secs_f64(), stdout_of(&out.stdout))
                }
                Some(b) => {
                    let mut spec = spec("build", base.path(), &tree);
                    // Only what a build needs of the builder's toolchain: not `~/.cargo` whole,
                    // which can hold a registry token.
                    spec.programs = ["bin", "registry", "git"]
                        .iter()
                        .map(|d| cargo_home.join(d))
                        .filter(|p| p.exists())
                        .chain([rustup_home.clone()])
                        .collect();
                    let sandbox = backends[b].1.create(spec).await.unwrap();
                    let t = Instant::now();
                    let child = spawn(&sandbox, build, &env(&target, sandbox.home()));
                    let out = child.wait_with_output().await.unwrap();
                    let secs = t.elapsed().as_secs_f64();
                    sandbox.destroy().await.unwrap();
                    (secs, stdout_of(&out.stdout))
                }
            };
            let label = places[i].map_or("outside", |b| backends[b].0);
            println!("round {round}: {label}: {secs:.1} s, {compiled} crates compiled");
            times[i].push(secs);
            std::fs::remove_dir_all(tree.join(&target)).unwrap();
        }
    }
    let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
    let outside = mean(&times[0]);
    println!("outside: mean {outside:.1} s");
    for (b, (label, _)) in backends.iter().enumerate() {
        let inside = mean(&times[b + 1]);
        println!(
            "{label}: mean {inside:.1} s, {:+.1}% against outside",
            (inside / outside - 1.0) * 100.0
        );
    }
}

fn stdout_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}
