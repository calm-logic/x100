//! The bubblewrap backend, for Linux and WSL.
//!
//! The sandbox is anchored by one process that `unshare` starts in new user, network, process and
//! mount namespaces: the helper's tunnel, as process 1 of the sandbox's process namespace, with
//! only loopback for a network. Each command enters those namespaces with `nsenter` and runs
//! under a fresh `bwrap` that builds its view of the files and gives it a process namespace of
//! its own inside the sandbox's. So commands share the sandbox's loopback (a server one starts is
//! reachable from the next) but stopping one ends everything it started, and killing the anchor
//! ends every process in the sandbox.
//!
//! Why `unshare` and not `bwrap` for the anchor: unprivileged bwrap 0.6 creates its network
//! namespace under an intermediate user namespace that no process stays in, so nothing could
//! join it later. `unshare` creates them all under one user namespace that the anchor stays in.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use crate::{
    Access, Backend, CommandLine, Enter, Error, Layout, Sandbox, SandboxSpec, io, mounts,
    sandbox_env,
};

/// Runs sandboxes with bubblewrap. Needs `bwrap`, `unshare`, `nsenter` and `ip` on the host, and
/// unprivileged user namespaces.
#[derive(Debug, Clone)]
pub struct Bubblewrap {
    /// The `x100-sandbox` helper program.
    pub helper: PathBuf,
}

impl Bubblewrap {
    pub fn new(helper: impl Into<PathBuf>) -> Self {
        Self {
            helper: helper.into(),
        }
    }
}

/// The host's top-level directories that are part of the system, each bound read-only, or made
/// the same link where the host has a link (`/bin` to `usr/bin` on a merged-usr system).
const SYSTEM: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc",
];

/// A fixed search path for the anchor, which starts from an empty environment.
const ANCHOR_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

#[async_trait]
impl Backend for Bubblewrap {
    async fn create(&self, spec: SandboxSpec) -> Result<Sandbox, Error> {
        let layout = Layout::make(&spec)?;
        let tools = (find("nsenter")?, find("bwrap")?);
        let anchor = Command::new("unshare")
            .args([
                "--user",
                "--map-root-user",
                "--net",
                "--pid",
                "--mount",
                "--mount-proc",
                "--fork",
                "--kill-child",
                "--",
                "/bin/sh",
                "-c",
                // The new network namespace starts with loopback down.
                "ip link set lo up && exec \"$0\" tunnel",
            ])
            .arg(&self.helper)
            .env_clear()
            .env("PATH", ANCHOR_PATH)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(io("starting unshare"))?;
        let pid = anchor.id().unwrap_or_default();
        let enter = Enterer {
            prefix: prefix(pid, tools, &spec, &layout),
        };
        Sandbox::assemble(
            spec,
            layout,
            Arc::new(enter),
            anchor,
            Duration::from_secs(10),
        )
        .await
    }
}

struct Enterer {
    /// Everything before the command itself.
    prefix: Vec<String>,
}

impl Enter for Enterer {
    fn command(&self, line: CommandLine) -> CommandLine {
        let mut args: Vec<std::ffi::OsString> = self.prefix[1..].iter().map(Into::into).collect();
        args.push(line.program.into_os_string());
        args.extend(line.args);
        CommandLine {
            program: PathBuf::from(&self.prefix[0]),
            args,
        }
    }

    fn teardown(&self) -> Option<std::process::Command> {
        None
    }
}

/// A program on the daemon's search path, so a command started with no `PATH` still runs.
pub(crate) fn find(program: &str) -> Result<String, Error> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join(program))
        .find(|p| p.is_file())
        .map(|p| p.display().to_string())
        .ok_or_else(|| Error::Spec(format!("`{program}` is not on the search path")))
}

fn prefix(
    anchor: u32,
    (nsenter, bwrap): (String, String),
    spec: &SandboxSpec,
    layout: &Layout,
) -> Vec<String> {
    let ns = |flag: &str, file: &str| format!("--{flag}=/proc/{anchor}/ns/{file}");
    let mut a: Vec<String> = vec![
        nsenter,
        "--preserve-credentials".into(),
        ns("user", "user"),
        ns("mount", "mnt"),
        ns("net", "net"),
        // The anchor's children, not the anchor itself, are in the sandbox's process namespace.
        format!("--pid=/proc/{anchor}/ns/pid_for_children"),
        "--".into(),
        bwrap,
        // The anchor's user namespace maps the builder to root; this one maps them back, with no
        // capabilities, so nothing inside can change its own mounts.
        "--unshare-user".into(),
        "--uid".into(),
        nix::unistd::getuid().to_string(),
        "--gid".into(),
        nix::unistd::getgid().to_string(),
        "--cap-drop".into(),
        "ALL".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--unshare-cgroup-try".into(),
        "--hostname".into(),
        "x100".into(),
        "--die-with-parent".into(),
    ];
    for dir in SYSTEM {
        let path = Path::new(dir);
        match std::fs::read_link(path) {
            Ok(target) => a.extend([
                "--symlink".into(),
                target.display().to_string(),
                (*dir).into(),
            ]),
            Err(_) if path.is_dir() => a.extend(["--ro-bind".into(), (*dir).into(), (*dir).into()]),
            Err(_) => {}
        }
    }
    a.extend(["--proc", "/proc", "--dev", "/dev"].map(String::from));
    a.extend([
        "--bind".into(),
        layout.tmp.display().to_string(),
        "/tmp".into(),
    ]);
    for m in mounts(spec, layout) {
        let flag = match m.access {
            Access::ReadOnly => "--ro-bind",
            Access::ReadWrite => "--bind",
        };
        let p = m.path.display().to_string();
        a.extend([flag.into(), p.clone(), p]);
    }
    // The skeleton bubblewrap built to hold the mounts is the sandbox's own, but nothing should
    // be written there either.
    a.extend(["--remount-ro".into(), "/".into()]);
    for (k, v) in sandbox_env(spec) {
        a.extend(["--setenv".into(), k, v]);
    }
    a.extend([
        "--chdir".into(),
        spec.worktree.display().to_string(),
        "--".into(),
    ]);
    a
}
