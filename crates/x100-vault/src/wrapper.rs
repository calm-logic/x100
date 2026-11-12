//! Running one named command with a credential in its environment.
//!
//! This narrows the routes to a credential; it is not a second sandbox. See the crate's
//! documentation for what it does not stop.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use x100_core::credential::{CredentialUse, Refusal, UseOutcome, Via};
use x100_core::id::CredentialRef;

use crate::redact::Redactor;
use crate::scope::Requester;
use crate::store::{Denied, Vault};

/// Programs that run files an agent can edit, refused whatever a credential's list says:
/// shells, programs that run other programs, build tools, package script runners, interpreters,
/// and tools whose own config or hooks live in the worktree (git).
const GENERAL_RUNNERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "mksh",
    "fish",
    "csh",
    "tcsh",
    "ash",
    "busybox",
    "env",
    "sudo",
    "doas",
    "su",
    "xargs",
    "nohup",
    "timeout",
    "nice",
    "ionice",
    "time",
    "watch",
    "setsid",
    "chroot",
    "flock",
    "parallel",
    "script",
    "make",
    "gmake",
    "bmake",
    "ninja",
    "cmake",
    "just",
    "task",
    "rake",
    "gradle",
    "gradlew",
    "mvn",
    "mvnw",
    "ant",
    "bazel",
    "bazelisk",
    "cargo",
    "go",
    "dotnet",
    "npm",
    "npx",
    "pnpm",
    "pnpx",
    "yarn",
    "bun",
    "bunx",
    "deno",
    "corepack",
    "pip",
    "pip3",
    "pipx",
    "uv",
    "uvx",
    "poetry",
    "pdm",
    "hatch",
    "tox",
    "nox",
    "composer",
    "bundle",
    "gem",
    "tsx",
    "ts-node",
    "java",
    "jshell",
    "kotlin",
    "scala",
    "groovy",
    "erl",
    "elixir",
    "iex",
    "mix",
    "ghc",
    "runghc",
    "stack",
    "swift",
    "pwsh",
    "powershell",
    "osascript",
    "awk",
    "gawk",
    "mawk",
    "sed",
    "Rscript",
    "R",
    "julia",
    "tclsh",
    "wish",
    "git",
    "pulumi",
    "cdk",
];

/// Interpreters that come with a version in their name, such as `python3.12`.
const RUNNER_PREFIXES: &[&str] = &["python", "pypy", "ruby", "perl", "php", "lua", "node"];

/// Deploy tools that run a build step from the project's own config, and that config's file
/// names. A name with a leading `*` matches by suffix. Files are matched by name anywhere in the
/// worktree, since each tool can be pointed at a config in a subdirectory.
const BUILT_IN_CONFIG: &[(&str, &[&str])] = &[
    (
        "wrangler",
        &[
            "wrangler.toml",
            "wrangler.json",
            "wrangler.jsonc",
            "package.json",
        ],
    ),
    ("vercel", &["vercel.json", "package.json"]),
    ("netlify", &["netlify.toml", "package.json"]),
    ("firebase", &["firebase.json", "package.json"]),
    ("fly", &["fly.toml", "Dockerfile"]),
    ("flyctl", &["fly.toml", "Dockerfile"]),
    (
        "terraform",
        &[
            "*.tf",
            "*.tf.json",
            "*.tfvars",
            ".terraformrc",
            "terraform.rc",
        ],
    ),
    ("tofu", &["*.tf", "*.tf.json", "*.tfvars", ".tofurc"]),
];

/// One run of one named command.
pub struct WrapRequest {
    pub requester: Requester,
    pub credential: CredentialRef,
    /// The variable the value is put in, such as `CLOUDFLARE_API_TOKEN`.
    pub env_var: String,
    /// A bare program name, looked up on `search_path`, never a path.
    pub command: String,
    pub args: Vec<String>,
    /// Where it runs: the agent's worktree.
    pub dir: PathBuf,
    /// The child's whole environment apart from the value. The daemon builds it; nothing of the
    /// agent's own environment goes in.
    pub env: Vec<(String, String)>,
    /// Where to look for the program. None of these may be writable by the agent.
    pub search_path: Vec<PathBuf>,
    /// Directories the agent can write to. A program found inside one is refused.
    pub writable: Vec<PathBuf>,
    /// Every file the agent's changes touch, relative to `dir`, untracked files included.
    pub changed: Vec<PathBuf>,
}

pub struct WrapOutput {
    pub status: Option<i32>,
    /// Redacted: every known value, in every form the redactor knows, is its reference.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The event to record.
    pub used: CredentialUse,
}

#[derive(Debug, thiserror::Error)]
pub enum WrapError {
    /// Refused; the use is still an event.
    #[error("refused: {:?}", .0.outcome)]
    Refused(CredentialUse),
    #[error("no credential {0}")]
    Unknown(CredentialRef),
    #[error("{0}")]
    BadRequest(String),
    #[error("starting {0}: {1}")]
    Spawn(String, std::io::Error),
}

pub struct Wrapper {
    vault: Vault,
    redactor: Redactor,
    config: BTreeMap<String, BTreeSet<String>>,
}

impl Wrapper {
    pub fn new(vault: Vault) -> Self {
        let config = BUILT_IN_CONFIG
            .iter()
            .map(|(tool, files)| {
                (
                    (*tool).to_owned(),
                    files.iter().map(|f| (*f).to_owned()).collect(),
                )
            })
            .collect();
        Self {
            redactor: vault.redactor(),
            vault,
            config,
        }
    }

    /// Adds config files for a tool, beyond the built-in table.
    pub fn with_config_files(
        mut self,
        tool: &str,
        files: impl IntoIterator<Item = String>,
    ) -> Self {
        self.config
            .entry(tool.to_owned())
            .or_default()
            .extend(files);
        self
    }

    pub async fn run(&self, req: WrapRequest) -> Result<WrapOutput, WrapError> {
        if req.command.is_empty() || req.command.contains('/') || req.command.starts_with('.') {
            return Err(WrapError::BadRequest(format!(
                "`{}` is not a program name",
                req.command
            )));
        }
        if !valid_env_name(&req.env_var) {
            return Err(WrapError::BadRequest(format!(
                "`{}` is not an environment variable name",
                req.env_var
            )));
        }
        let via = Via::Wrapper {
            command: req.command.clone(),
        };
        let event = |outcome| CredentialUse {
            credential: req.credential,
            agent: req.requester.agent,
            driver: req.requester.driver,
            via: via.clone(),
            outcome,
        };
        let refused = |r| WrapError::Refused(event(UseOutcome::Refused(r)));
        if general_runner(&req.command) {
            return Err(refused(Refusal::CommandNotAllowed));
        }
        let value = match self.vault.take(req.credential, &req.requester, &via) {
            Ok(value) => value,
            Err(Denied::Unknown) => return Err(WrapError::Unknown(req.credential)),
            Err(Denied::Refused(r)) => return Err(refused(r)),
        };
        if let Some(files) = self.config.get(&req.command)
            && req.changed.iter().any(|c| touches(files, c))
        {
            return Err(refused(Refusal::ConfigTouched));
        }
        let Some(program) = find(&req.command, &req.search_path) else {
            return Err(WrapError::BadRequest(format!(
                "`{}` is not on the search path",
                req.command
            )));
        };
        if req.writable.iter().any(|w| inside(&program, w)) {
            return Err(refused(Refusal::CommandNotAllowed));
        }
        let mut child = Command::new(&program)
            .args(&req.args)
            .current_dir(&req.dir)
            .env_clear()
            .envs(req.env.iter().map(|(k, v)| (k, v)))
            .env(&req.env_var, OsStr::from_bytes(value.expose()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| WrapError::Spawn(req.command.clone(), e))?;
        drop(value);
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (stdout, stderr, status) =
            tokio::join!(self.drain(stdout), self.drain(stderr), child.wait());
        let status = status.map_err(|e| WrapError::Spawn(req.command.clone(), e))?;
        Ok(WrapOutput {
            status: status.code(),
            stdout,
            stderr,
            used: event(UseOutcome::Injected),
        })
    }

    /// Reads a pipe to its end through a stream redactor, so a value split across reads is
    /// still caught.
    async fn drain(&self, mut pipe: impl AsyncRead + Unpin) -> Vec<u8> {
        let mut stream = self.redactor.stream();
        let mut out = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(n) = pipe.read(&mut buf).await {
            if n == 0 {
                break;
            }
            out.extend(stream.push(&buf[..n]));
        }
        out.extend(stream.finish());
        out
    }
}

fn general_runner(command: &str) -> bool {
    GENERAL_RUNNERS.contains(&command)
        || RUNNER_PREFIXES.iter().any(|p| {
            command
                .strip_prefix(p)
                .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
        })
}

fn touches(files: &BTreeSet<String>, changed: &Path) -> bool {
    let Some(name) = changed.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    files.iter().any(|f| match f.strip_prefix('*') {
        Some(suffix) => name.ends_with(suffix),
        None => name == f,
    })
}

fn find(command: &str, search_path: &[PathBuf]) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    search_path
        .iter()
        .map(|dir| dir.join(command))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .and_then(|p| std::fs::canonicalize(p).ok())
}

/// Compared after resolving links, so a link out of the worktree to a program in it is caught.
fn inside(program: &Path, dir: &Path) -> bool {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    program.starts_with(dir)
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runners_are_known_by_name_and_version() {
        for name in [
            "bash",
            "make",
            "npm",
            "python3",
            "python3.12",
            "node22",
            "git",
            "cargo",
        ] {
            assert!(general_runner(name), "{name}");
        }
        for name in ["wrangler", "flyctl", "pythonista", "homebox"] {
            assert!(!general_runner(name), "{name}");
        }
    }

    #[test]
    fn config_files_match_by_name_anywhere() {
        let files: BTreeSet<String> = ["wrangler.toml".to_owned(), "*.tf".to_owned()].into();
        assert!(touches(&files, Path::new("wrangler.toml")));
        assert!(touches(&files, Path::new("apps/site/wrangler.toml")));
        assert!(touches(&files, Path::new("infra/main.tf")));
        assert!(!touches(&files, Path::new("src/index.ts")));
        assert!(!touches(&files, Path::new("main.tfstate")));
    }
}
