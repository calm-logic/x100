//! The container backend, on Docker or Podman: the default on macOS and an option on Linux.
//!
//! The sandbox is one container with no network, anchored by the helper's tunnel as its first
//! process, attached to the daemon by the runtime's standard input and output. That works the
//! same where the runtime runs in a virtual machine (macOS), where neither a host port without
//! a network nor a Unix socket in a shared directory would. Each command runs through the
//! helper's `exec` on the host, which starts it with `<runtime> exec` under the helper's `run`
//! inside. A runtime does not stop what `exec` started when its client is killed, so `run` keeps
//! a record of the command, and `exec` has it end the command, and everything the command
//! started, when the client is told to stop.

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

/// The base image: the system a driver needs, and nothing of the builder's. The programs and
/// toolchains come from the host as read-only mounts, which on Linux run as they are. Remove it
/// with `docker rmi x100-sandbox-base:1` (or `podman rmi`).
pub const IMAGE: &str = "x100-sandbox-base:1";

/// The same release as this machine's host, so programs bound in from the host find the C
/// library they were built against.
const DOCKERFILE: &str = "FROM ubuntu:22.04\n\
RUN apt-get update \\\n \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \\\n \
    git ca-certificates build-essential pkg-config curl \\\n \
 && rm -rf /var/lib/apt/lists/*\n";

/// What the runtime's own command line needs from the daemon's environment to find its engine.
const RUNTIME_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "CONTAINER_HOST",
];

#[derive(Debug, Clone)]
pub struct Runtime {
    pub program: PathBuf,
    /// Podman maps the builder's own user id into a rootless container only when asked.
    pub podman: bool,
}

impl Runtime {
    pub fn docker() -> Self {
        Self {
            program: PathBuf::from("docker"),
            podman: false,
        }
    }

    pub fn podman() -> Self {
        Self {
            program: PathBuf::from("podman"),
            podman: true,
        }
    }

    fn env() -> Vec<(String, String)> {
        RUNTIME_ENV
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_owned(), v)))
            .collect()
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.env_clear().envs(Self::env());
        c
    }
}

#[derive(Debug, Clone)]
pub struct Container {
    pub runtime: Runtime,
    pub image: String,
    /// The `x100-sandbox` helper program, built for Linux. It is mounted into each container.
    pub helper: PathBuf,
}

impl Container {
    pub fn new(runtime: Runtime, helper: impl Into<PathBuf>) -> Self {
        Self {
            runtime,
            image: IMAGE.to_owned(),
            helper: helper.into(),
        }
    }

    /// Builds the base image if the runtime does not have it yet.
    pub async fn ensure_image(&self) -> Result<(), Error> {
        let have = self
            .runtime
            .command()
            .args(["image", "inspect", &self.image])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(io("asking the runtime for the image"))?;
        if have.success() {
            return Ok(());
        }
        let mut build = self
            .runtime
            .command()
            .args(["build", "-q", "-t", &self.image, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(io("building the base image"))?;
        if let Some(mut stdin) = build.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(DOCKERFILE.as_bytes())
                .await
                .map_err(io("writing the Dockerfile"))?;
        }
        let out = build
            .wait_with_output()
            .await
            .map_err(io("building the base image"))?;
        if !out.status.success() {
            return Err(Error::Spec(format!(
                "building {} failed: {}",
                self.image,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Backend for Container {
    async fn create(&self, spec: SandboxSpec) -> Result<Sandbox, Error> {
        let layout = Layout::make(&spec)?;
        let env_dir = spec.dir.join("env");
        std::fs::create_dir_all(&env_dir).map_err(io("making the environment directory"))?;
        let name = format!("x100-sandbox-{}", spec.name);
        let helper = self.helper.display().to_string();
        let mut args: Vec<String> = [
            "run",
            "-i",
            "--rm",
            "--read-only",
            "--name",
            &name,
            "--network",
            "none",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--hostname",
            "x100",
            "--user",
        ]
        .map(String::from)
        .into();
        args.push(format!(
            "{}:{}",
            nix::unistd::getuid(),
            nix::unistd::getgid()
        ));
        if self.runtime.podman {
            args.push("--userns=keep-id".into());
        }
        args.extend(["--workdir".into(), spec.worktree.display().to_string()]);
        args.extend(bind(&layout.tmp, Path::new("/tmp"), Access::ReadWrite)?);
        args.extend(bind(&self.helper, &self.helper, Access::ReadOnly)?);
        for m in mounts(&spec, &layout) {
            args.extend(bind(&m.path, &m.path, m.access)?);
        }
        args.extend([self.image.clone(), helper.clone(), "tunnel".into()]);
        let anchor = self
            .runtime
            .command()
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(io("starting the container"))?;
        let mut prefix: Vec<String> = vec![
            "exec".into(),
            "--runtime".into(),
            self.runtime.program.display().to_string(),
            "--name".into(),
            name.clone(),
            "--helper".into(),
            helper,
            "--env-dir".into(),
            env_dir.display().to_string(),
            "--cwd".into(),
            spec.worktree.display().to_string(),
        ];
        for (k, v) in sandbox_env(&spec) {
            prefix.extend(["--set".into(), format!("{k}={v}")]);
        }
        for (k, v) in Runtime::env() {
            prefix.extend(["--runtime-env".into(), format!("{k}={v}")]);
        }
        prefix.push("--".into());
        let enter = Enterer {
            helper: self.helper.clone(),
            prefix,
            runtime: self.runtime.clone(),
            name,
        };
        // A runtime in a virtual machine can take seconds to start a container.
        Sandbox::assemble(
            spec,
            layout,
            Arc::new(enter),
            anchor,
            Duration::from_secs(60),
        )
        .await
    }
}

fn bind(from: &Path, to: &Path, access: Access) -> Result<[String; 2], Error> {
    let (from, to) = (from.display().to_string(), to.display().to_string());
    if from.contains(',') || to.contains(',') {
        return Err(Error::Spec(format!("{from}: a comma cannot be mounted")));
    }
    let ro = match access {
        Access::ReadOnly => ",readonly",
        Access::ReadWrite => "",
    };
    Ok([
        "--mount".into(),
        format!("type=bind,source={from},target={to}{ro}"),
    ])
}

struct Enterer {
    helper: PathBuf,
    prefix: Vec<String>,
    runtime: Runtime,
    name: String,
}

impl Enter for Enterer {
    fn command(&self, line: CommandLine) -> CommandLine {
        let mut args: Vec<std::ffi::OsString> = self.prefix.iter().map(Into::into).collect();
        args.push(line.program.into_os_string());
        args.extend(line.args);
        CommandLine {
            program: self.helper.clone(),
            args,
        }
    }

    fn teardown(&self) -> Option<std::process::Command> {
        let mut c = std::process::Command::new(&self.runtime.program);
        c.env_clear()
            .envs(Runtime::env())
            // SIGKILL at once: as process 1 the helper ignores the SIGTERM that `podman rm -f` sends
            // first and waits on. `--rm` then removes the container.
            .args(["kill", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Some(c)
    }
}
