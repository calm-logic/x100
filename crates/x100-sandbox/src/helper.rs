//! The `x100-sandbox` helper program, which the sandbox runs on both sides of its boundary:
//!
//! - `tunnel`, inside: the sandbox's anchor and the inside end of [`crate::tunnel`]. As the first
//!   process of the sandbox it also reaps what others leave behind.
//! - `exec`, on the host, container backend only: starts one command in the container with the
//!   environment the caller gave it, and ends it when told to stop.
//! - `run` and `kill`, inside a container: run one command and end it with everything it
//!   started, which a runtime does not do when its `exec` client is killed.

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use tokio::signal::unix::{SignalKind, signal};

/// Where a container's `run` records the commands it runs, by tag.
const RUNS: &str = "/tmp/.x100-run";

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(&format!("no runtime: {e}")),
    };
    match args.first().map(String::as_str) {
        Some("tunnel") => runtime.block_on(tunnel()),
        Some("exec") => runtime.block_on(exec(&args[1..])),
        Some("run") => runtime.block_on(run(&args[1..])),
        Some("kill") => end(&args[1..]),
        _ => fail("usage: x100-sandbox tunnel | exec ... | run --tag T -- CMD | kill T"),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("x100-sandbox: {message}");
    ExitCode::from(125)
}

async fn tunnel() -> ExitCode {
    tokio::spawn(async {
        let Ok(mut children) = signal(SignalKind::child()) else {
            return;
        };
        while children.recv().await.is_some() {
            reap_all();
        }
    });
    match crate::tunnel::inside(tokio::io::stdin(), tokio::io::stdout()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("tunnel: {e}")),
    }
}

/// Reaps every child that has exited, returning their statuses.
fn reap_all() -> Vec<WaitStatus> {
    let mut reaped = Vec::new();
    while let Ok(status) = waitpid(None, Some(WaitPidFlag::WNOHANG)) {
        if status == WaitStatus::StillAlive {
            break;
        }
        reaped.push(status);
    }
    reaped
}

/// The command's options, then `--`, then the command.
struct Parsed {
    options: Vec<(String, String)>,
    command: Vec<String>,
}

fn parse(args: &[String]) -> Option<Parsed> {
    let split = args.iter().position(|a| a == "--")?;
    let (options, command) = (&args[..split], &args[split + 1..]);
    if options.len() % 2 != 0 || command.is_empty() {
        return None;
    }
    let options = options
        .chunks(2)
        .map(|p| (p[0].trim_start_matches("--").to_owned(), p[1].clone()))
        .collect();
    Some(Parsed {
        options,
        command: command.to_vec(),
    })
}

impl Parsed {
    fn one(&self, name: &str) -> Option<&str> {
        self.options
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> impl Iterator<Item = (&str, &str)> {
        self.options
            .iter()
            .filter(move |(k, _)| k == name)
            .filter_map(|(_, v)| v.split_once('='))
    }
}

async fn exec(args: &[String]) -> ExitCode {
    let Some(p) = parse(args) else {
        return fail("exec: bad arguments");
    };
    let (Some(runtime), Some(name), Some(helper), Some(env_dir), Some(cwd)) = (
        p.one("runtime"),
        p.one("name"),
        p.one("helper"),
        p.one("env-dir"),
        p.one("cwd"),
    ) else {
        return fail("exec: missing an option");
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let tag = format!("{}-{nanos}", std::process::id());
    // The command's environment goes in a file only the builder can read, not on a command line
    // anyone can see.
    let env_file = PathBuf::from(env_dir).join(format!("{tag}.env"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&env_file)
        .and_then(|mut f| {
            let vars = std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
            for (k, v) in vars {
                if !v.contains('\n') {
                    writeln!(f, "{k}={v}")?;
                }
            }
            Ok(())
        });
    if let Err(e) = written {
        return fail(&format!("exec: writing the environment: {e}"));
    }
    let runtime_env: Vec<(&str, &str)> = p.all("runtime-env").collect();
    let mut command = tokio::process::Command::new(runtime);
    command.env_clear().envs(runtime_env.iter().copied());
    command.arg("exec").arg("-i");
    if std::io::stdin().is_terminal() {
        command.arg("-t");
    }
    command.arg("--env-file").arg(&env_file).args(["-w", cwd]);
    for (k, v) in p.all("set") {
        command.arg("-e").arg(format!("{k}={v}"));
    }
    command
        .args([name, helper, "run", "--tag", &tag, "--"])
        .args(&p.command);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&env_file);
            return fail(&format!("exec: starting {runtime}: {e}"));
        }
    };
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        return fail("exec: cannot handle signals");
    };
    let status = tokio::select! {
        s = child.wait() => s.ok(),
        _ = term.recv() => None,
        _ = int.recv() => None,
        _ = hup.recv() => None,
    };
    let status = match status {
        Some(s) => s.code(),
        None => {
            let _ = tokio::process::Command::new(runtime)
                .env_clear()
                .envs(runtime_env.iter().copied())
                .args(["exec", name, helper, "kill", &tag])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                Ok(Ok(s)) => s.code(),
                _ => {
                    let _ = child.kill().await;
                    None
                }
            }
        }
    };
    let _ = std::fs::remove_file(&env_file);
    ExitCode::from(status.and_then(|c| u8::try_from(c).ok()).unwrap_or(1))
}

/// How long a command has after SIGTERM before it and all it started are killed.
const GRACE: Duration = Duration::from_secs(3);

async fn run(args: &[String]) -> ExitCode {
    let Some(p) = parse(args) else {
        return fail("run: bad arguments");
    };
    let Some(tag) = p.one("tag") else {
        return fail("run: no tag");
    };
    // Whatever the command starts and abandons, a process that left its session included,
    // becomes this process's child, so it can be found and ended.
    if let Err(e) = nix::sys::prctl::set_child_subreaper(true) {
        return fail(&format!("run: cannot become a subreaper: {e}"));
    }
    let record = PathBuf::from(RUNS).join(tag);
    let _ = std::fs::create_dir_all(RUNS);
    if let Err(e) = std::fs::write(&record, std::process::id().to_string()) {
        return fail(&format!("run: recording the command: {e}"));
    }
    let child = match std::process::Command::new(&p.command[0])
        .args(&p.command[1..])
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&record);
            return fail(&format!("run: starting {}: {e}", p.command[0]));
        }
    };
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
        signal(SignalKind::hangup()),
    ) else {
        return fail("run: cannot handle signals");
    };
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut code: Option<u8> = None;
    while code.is_none() {
        tokio::select! {
            _ = tick.tick() => {}
            _ = term.recv() => deadline = deadline.or(Some(stop(pid))),
            _ = int.recv() => deadline = deadline.or(Some(stop(pid))),
            _ = hup.recv() => deadline = deadline.or(Some(stop(pid))),
        }
        for status in reap_all() {
            code = match status {
                WaitStatus::Exited(p, c) if p == pid => Some(u8::try_from(c).unwrap_or(1)),
                WaitStatus::Signaled(p, s, _) if p == pid => Some(128u8.saturating_add(s as u8)),
                _ => code,
            };
        }
        if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
            code = Some(137);
        }
    }
    kill_descendants();
    let _ = std::fs::remove_file(&record);
    ExitCode::from(code.unwrap_or(1))
}

fn stop(pid: Pid) -> tokio::time::Instant {
    let _ = kill(pid, Signal::SIGTERM);
    tokio::time::Instant::now() + GRACE
}

/// Kills everything below this process until nothing is left, so a command that forks while it
/// is being killed is still caught.
fn kill_descendants() {
    let me = std::process::id();
    for _ in 0..50 {
        let below = descendants(me);
        if below.is_empty() {
            return;
        }
        for pid in below {
            if let Ok(pid) = i32::try_from(pid) {
                let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
        reap_all();
    }
}

fn descendants(root: u32) -> Vec<u32> {
    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse().ok()) else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            // The name in brackets may hold spaces and brackets of its own.
            let ppid = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok());
            if let Some(ppid) = ppid {
                parent_of.insert(pid, ppid);
            }
        }
    }
    let mut found: HashSet<u32> = HashSet::new();
    let mut frontier = vec![root];
    while let Some(p) = frontier.pop() {
        for (child, parent) in &parent_of {
            if *parent == p && found.insert(*child) {
                frontier.push(*child);
            }
        }
    }
    found.into_iter().collect()
}

fn end(args: &[String]) -> ExitCode {
    let Some(tag) = args.first() else {
        return fail("kill: no tag");
    };
    if tag.contains('/') {
        return fail("kill: bad tag");
    }
    let pid = std::fs::read_to_string(PathBuf::from(RUNS).join(tag))
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok());
    match pid {
        Some(pid) => {
            let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
            ExitCode::SUCCESS
        }
        None => fail("kill: no such command"),
    }
}
