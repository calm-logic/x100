use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use x100_core::time::{Clock, FakeClock, Timestamp};
use x100_jobs::SystemClock;

/// The most a client may send on one line. A file travels as base64 in one request, so this is
/// also the largest upload: about 12 MiB of file. The protocol allows 64 MiB; a screenshot or a PDF
/// fits well under this, and a connection can hold at most this much of a half-read line.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// How many messages a connection may have waiting to be written before a subscription on it is
/// cut off as lagging. The kernel's socket buffer takes the first part of a slow reader's backlog.
pub const QUEUE_CAPACITY: usize = 4096;

/// How long a client may leave a full queue unread before it is cut off. A client that is reading
/// frees room constantly, so only one that has stopped waits this long.
pub const LAG_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

pub const DATA_DIR_ENV: &str = "X100_DATA_DIR";
pub const PROJECTS_ROOT_ENV: &str = "X100_PROJECTS_ROOT";
pub const SCRIPTS_ENV: &str = "X100_FAKE_SCRIPTS";
pub const FAKE_PROGRAM_ENV: &str = "X100_FAKE_HARNESS";
pub const CLAUDE_LOGIN_ENV: &str = "X100_CLAUDE_LOGIN";
pub const CODEX_LOGIN_ENV: &str = "X100_CODEX_LOGIN";
/// Where each provider is reached, if not at its own address: the variables the two programs
/// themselves read, passed on to the drivers that run them in a terminal.
pub const CLAUDE_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
pub const CODEX_BASE_URL_ENV: &str = "OPENAI_BASE_URL";
pub const SANDBOX_ENV: &str = "X100_SANDBOX";
pub const SANDBOX_HELPER_ENV: &str = "X100_SANDBOX_HELPER";
/// Any value: the vault keeps its key in a file and never asks the OS keychain. For tests.
pub const NO_KEYCHAIN_ENV: &str = "X100_NO_KEYCHAIN";
/// `host=address,...`: the proxy sends each host to the address given and refuses every other
/// host. For tests, so nothing a driver tries leaves the machine. An address may name a port,
/// `host=127.0.0.1:8443`, which is used instead of the one asked for: the daemon's own calls to a
/// decision model ask for 443.
pub const PROXY_RESOLVE_ENV: &str = "X100_PROXY_RESOLVE";
/// A PEM file of the only authorities the proxy trusts upstream, instead of the system's. Taken
/// only with `X100_PROXY_RESOLVE`, so it can only point the proxy at a test's own servers.
pub const PROXY_UPSTREAM_CA_ENV: &str = "X100_PROXY_UPSTREAM_CA";

/// What makes the sandboxes drivers run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxKind {
    Bubblewrap,
    Docker,
    Podman,
}

impl SandboxKind {
    /// Bubblewrap where it exists, which is Linux; containers elsewhere.
    pub fn default_here() -> Self {
        if cfg!(target_os = "linux") {
            SandboxKind::Bubblewrap
        } else {
            SandboxKind::Docker
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "bubblewrap" => SandboxKind::Bubblewrap,
            "docker" => SandboxKind::Docker,
            "podman" => SandboxKind::Podman,
            _ => return None,
        })
    }
}

#[derive(Clone)]
pub struct Config {
    /// Holds the daemon's log, the socket, the list of driver processes, and each project's own
    /// store under `projects/<id>`.
    pub data_dir: PathBuf,
    /// Where a project's code directory is made when it is set up, unless the daemon's setting
    /// names another: `~/x100` unless named.
    pub projects_root: PathBuf,
    /// A directory of fake-harness scripts (`*.toml`). When given, those are the only harnesses:
    /// the daemon looks for no real one.
    pub scripts_dir: Option<PathBuf>,
    /// The program that plays a script. Defaults to `x100d-fake-harness` beside this executable.
    pub fake_program: Option<PathBuf>,
    pub max_line: usize,
    pub queue_capacity: usize,
    /// How long a connection's queue may stay full before its subscription is cut off.
    pub lag_after: std::time::Duration,
    pub clock: Arc<dyn Clock>,
    /// Set by `--fake-clock`: `clock` is this one, which moves only by `daemon.set_clock`, and
    /// each project runs one turn at a time, so a run of fake-harness scripts is the same every
    /// time. For tests and scripted runs.
    pub fake_clock: Option<FakeClock>,
    /// The builder's Claude Code login directory, which terminal drivers of Claude Code run on:
    /// `~/.claude` unless named. The daemon checks that it exists and never opens what is in it.
    pub claude_login: Option<PathBuf>,
    /// The builder's Codex login directory, `~/.codex` unless named.
    pub codex_login: Option<PathBuf>,
    pub claude_base_url: Option<String>,
    pub codex_base_url: Option<String>,
    pub sandbox: SandboxKind,
    /// The sandbox's helper program. Defaults to `x100d-sandbox` beside this executable.
    pub sandbox_helper: Option<PathBuf>,
    /// The vault's key is a file in the data directory, never in the OS keychain.
    pub no_keychain: bool,
    /// Hosts the proxy sends to a fixed address, refusing every other. Empty: the proxy looks
    /// hosts up and refuses none but the host's loopback.
    pub proxy_resolve: std::collections::HashMap<String, x100_vault::Resolved>,
    /// The authorities the proxy trusts upstream instead of the system's, with `proxy_resolve`.
    pub proxy_upstream_ca: Option<PathBuf>,
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self {
            data_dir: data_dir.into(),
            projects_root: projects_root(home),
            scripts_dir: None,
            fake_program: None,
            max_line: MAX_LINE,
            queue_capacity: QUEUE_CAPACITY,
            lag_after: LAG_AFTER,
            clock: Arc::new(SystemClock),
            fake_clock: None,
            claude_login: home_dir(".claude", |k| std::env::var_os(k)),
            codex_login: home_dir(".codex", |k| std::env::var_os(k)),
            claude_base_url: None,
            codex_base_url: None,
            sandbox: SandboxKind::default_here(),
            sandbox_helper: None,
            no_keychain: false,
            proxy_resolve: std::collections::HashMap::new(),
            proxy_upstream_ca: None,
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        self.data_dir.join("x100d.sock")
    }

    /// From `--data-dir`, `--projects-root`, `--scripts-dir`, `--fake-harness`, `--claude-login`,
    /// `--codex-login`, `--sandbox` (`bubblewrap`, `docker` or `podman`) and `--sandbox-helper`
    /// arguments, each of which falls back to its environment variable, and `--fake-clock`. The data directory then falls back to the user's data directory, the
    /// projects root to `~/x100`, and each login to the program's own directory in the user's
    /// home.
    pub fn from_args(
        args: impl IntoIterator<Item = OsString>,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, String> {
        let (mut data, mut scripts, mut fake, mut clock) = (None, None, None, None);
        let (mut claude, mut codex, mut root) = (None, None, None);
        let (mut sandbox, mut helper) = (None, None);
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let slot = match arg.to_str() {
                Some("--data-dir") => &mut data,
                Some("--projects-root") => &mut root,
                Some("--scripts-dir") => &mut scripts,
                Some("--fake-harness") => &mut fake,
                Some("--fake-clock") => &mut clock,
                Some("--claude-login") => &mut claude,
                Some("--codex-login") => &mut codex,
                Some("--sandbox") => &mut sandbox,
                Some("--sandbox-helper") => &mut helper,
                _ => return Err(format!("unknown argument {arg:?}")),
            };
            let value = args
                .next()
                .ok_or_else(|| format!("{arg:?} needs a value"))?;
            *slot = Some(value);
        }
        let data_dir = data
            .map(PathBuf::from)
            .or_else(|| env(DATA_DIR_ENV).map(PathBuf::from))
            .or_else(default_data_dir)
            .ok_or("no data directory: set --data-dir or X100_DATA_DIR")?;
        let mut config = Config::new(data_dir);
        config.projects_root = root
            .or_else(|| env(PROJECTS_ROOT_ENV))
            .map(PathBuf::from)
            .unwrap_or_else(|| projects_root(env("HOME").map(PathBuf::from)));
        config.scripts_dir = scripts
            .map(PathBuf::from)
            .or_else(|| env(SCRIPTS_ENV).map(PathBuf::from));
        config.fake_program = fake
            .map(PathBuf::from)
            .or_else(|| env(FAKE_PROGRAM_ENV).map(PathBuf::from));
        config.claude_login = claude
            .or_else(|| env(CLAUDE_LOGIN_ENV))
            .map(PathBuf::from)
            .or_else(|| home_dir(".claude", &env));
        config.codex_login = codex
            .or_else(|| env(CODEX_LOGIN_ENV))
            .map(PathBuf::from)
            .or_else(|| home_dir(".codex", &env));
        let text = |k: &str| env(k).and_then(|v| v.into_string().ok());
        config.claude_base_url = text(CLAUDE_BASE_URL_ENV);
        config.codex_base_url = text(CODEX_BASE_URL_ENV);
        if let Some(kind) = sandbox.or_else(|| env(SANDBOX_ENV)) {
            config.sandbox = kind
                .to_str()
                .and_then(SandboxKind::parse)
                .ok_or("--sandbox is bubblewrap, docker or podman")?;
        }
        config.sandbox_helper = helper
            .or_else(|| env(SANDBOX_HELPER_ENV))
            .map(PathBuf::from);
        config.no_keychain = env(NO_KEYCHAIN_ENV).is_some();
        if let Some(pairs) = text(PROXY_RESOLVE_ENV) {
            config.proxy_resolve = resolve(&pairs)?;
        }
        if let Some(ca) = env(PROXY_UPSTREAM_CA_ENV) {
            if config.proxy_resolve.is_empty() {
                return Err(format!(
                    "{PROXY_UPSTREAM_CA_ENV} is taken only with {PROXY_RESOLVE_ENV}"
                ));
            }
            config.proxy_upstream_ca = Some(PathBuf::from(ca));
        }
        if let Some(start) = clock {
            // A real harness bills real time, so only scripted ones run on a fake clock.
            if config.scripts_dir.is_none() {
                return Err(
                    "--fake-clock needs --scripts-dir: only fake harnesses run on it".into(),
                );
            }
            let start: u64 = start
                .to_str()
                .and_then(|s| s.parse().ok())
                .ok_or("--fake-clock needs a time in milliseconds since 1970")?;
            let fake = FakeClock::new(Timestamp::from_millis(start));
            config.clock = Arc::new(fake.clone());
            config.fake_clock = Some(fake);
        }
        Ok(config)
    }
}

/// `host=address,host=address:port`.
fn resolve(pairs: &str) -> Result<std::collections::HashMap<String, x100_vault::Resolved>, String> {
    pairs
        .split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (host, at) = p
                .split_once('=')
                .ok_or_else(|| format!("{PROXY_RESOLVE_ENV}: {p:?} is not host=address"))?;
            let at = at.trim();
            let resolved = match at.parse::<std::net::SocketAddr>() {
                Ok(socket) => x100_vault::Resolved {
                    ip: socket.ip(),
                    port: Some(socket.port()),
                },
                Err(_) => at
                    .parse::<std::net::IpAddr>()
                    .map(x100_vault::Resolved::from)
                    .map_err(|_| format!("{PROXY_RESOLVE_ENV}: {at:?} is not an address"))?,
            };
            Ok((host.trim().to_ascii_lowercase(), resolved))
        })
        .collect()
}

/// `~/x100`, where new projects' code goes by default. Without a home, the current directory's
/// `x100`.
fn projects_root(home: Option<PathBuf>) -> PathBuf {
    home.unwrap_or_default().join("x100")
}

/// `name` in the user's home, where each program keeps its login by default.
fn home_dir(name: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    env("HOME").map(|h| PathBuf::from(h).join(name))
}

/// `$XDG_DATA_HOME/x100`, or `~/.local/share/x100`. The client library derives the same path.
pub fn default_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("x100"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn arguments_beat_the_environment() {
        let env = |k: &str| (k == DATA_DIR_ENV).then(|| OsString::from("/from/env"));
        let c = Config::from_args(["--data-dir".into(), "/from/arg".into()], env).unwrap();
        assert_eq!(c.data_dir, PathBuf::from("/from/arg"));
        let c = Config::from_args([], env).unwrap();
        assert_eq!(c.data_dir, PathBuf::from("/from/env"));
    }

    #[test]
    fn scripts_come_from_an_argument_or_the_environment() {
        let c = Config::from_args(
            [
                "--data-dir".into(),
                "/d".into(),
                "--scripts-dir".into(),
                "/s".into(),
            ],
            no_env,
        )
        .unwrap();
        assert_eq!(c.scripts_dir, Some(PathBuf::from("/s")));
        let env = |k: &str| (k == SCRIPTS_ENV).then(|| OsString::from("/e"));
        let c = Config::from_args(["--data-dir".into(), "/d".into()], env).unwrap();
        assert_eq!(c.scripts_dir, Some(PathBuf::from("/e")));
    }

    #[test]
    fn a_fake_clock_starts_where_it_is_told_and_only_with_fake_harnesses() {
        let args = |extra: &[&str]| -> Vec<OsString> {
            ["--data-dir", "/d"]
                .iter()
                .chain(extra)
                .map(OsString::from)
                .collect()
        };
        let c = Config::from_args(
            args(&["--scripts-dir", "/s", "--fake-clock", "1794387600000"]),
            no_env,
        )
        .unwrap();
        assert_eq!(c.clock.now(), Timestamp::from_millis(1_794_387_600_000));
        assert!(c.fake_clock.is_some());
        assert!(Config::from_args(args(&["--fake-clock", "1794387600000"]), no_env).is_err());
        assert!(
            Config::from_args(
                args(&["--scripts-dir", "/s", "--fake-clock", "soon"]),
                no_env
            )
            .is_err()
        );
        assert!(
            Config::from_args(args(&[]), no_env)
                .unwrap()
                .fake_clock
                .is_none()
        );
    }

    #[test]
    fn each_login_is_the_programs_own_directory_unless_named() {
        let home = |k: &str| (k == "HOME").then(|| OsString::from("/home/b"));
        let c = Config::from_args(["--data-dir".into(), "/d".into()], home).unwrap();
        assert_eq!(c.claude_login, Some(PathBuf::from("/home/b/.claude")));
        assert_eq!(c.codex_login, Some(PathBuf::from("/home/b/.codex")));
        let named = |k: &str| match k {
            "HOME" => Some(OsString::from("/home/b")),
            CODEX_LOGIN_ENV => Some(OsString::from("/elsewhere/codex")),
            _ => None,
        };
        let args = ["--data-dir", "/d", "--claude-login", "/throwaway/claude"];
        let c = Config::from_args(args.map(OsString::from), named).unwrap();
        assert_eq!(c.claude_login, Some(PathBuf::from("/throwaway/claude")));
        assert_eq!(c.codex_login, Some(PathBuf::from("/elsewhere/codex")));
    }

    #[test]
    fn the_projects_root_is_named_or_x100_in_the_home() {
        let home = |k: &str| (k == "HOME").then(|| OsString::from("/home/b"));
        let c = Config::from_args(["--data-dir".into(), "/d".into()], home).unwrap();
        assert_eq!(c.projects_root, PathBuf::from("/home/b/x100"));
        let args = ["--data-dir", "/d", "--projects-root", "/code"];
        let c = Config::from_args(args.map(OsString::from), home).unwrap();
        assert_eq!(c.projects_root, PathBuf::from("/code"));
        let env = |k: &str| (k == PROJECTS_ROOT_ENV).then(|| OsString::from("/env"));
        let c = Config::from_args(["--data-dir".into(), "/d".into()], env).unwrap();
        assert_eq!(c.projects_root, PathBuf::from("/env"));
    }

    #[test]
    fn the_sandbox_and_the_proxys_hosts_are_named_or_the_defaults() {
        let c = Config::from_args(["--data-dir".into(), "/d".into()], no_env).unwrap();
        assert_eq!(c.sandbox, SandboxKind::default_here());
        assert!(c.proxy_resolve.is_empty() && !c.no_keychain);
        let env = |k: &str| match k {
            PROXY_RESOLVE_ENV => Some(OsString::from("A.test=127.0.0.1, b.test=::1")),
            NO_KEYCHAIN_ENV => Some(OsString::from("1")),
            _ => None,
        };
        let args = ["--data-dir", "/d", "--sandbox", "podman"];
        let c = Config::from_args(args.map(OsString::from), env).unwrap();
        assert_eq!(c.sandbox, SandboxKind::Podman);
        assert!(c.no_keychain);
        assert_eq!(c.proxy_resolve.len(), 2);
        assert!(c.proxy_resolve.contains_key("a.test"));
        assert_eq!(c.proxy_resolve["b.test"].port, None);
        // A port, for a daemon's own call to a server a test runs; and the test's authority with it.
        let env = |k: &str| match k {
            PROXY_RESOLVE_ENV => Some(OsString::from("a.test=127.0.0.1:8443,b.test=[::1]:9")),
            PROXY_UPSTREAM_CA_ENV => Some(OsString::from("/t/ca.pem")),
            _ => None,
        };
        let c = Config::from_args(["--data-dir".into(), "/d".into()], env).unwrap();
        assert_eq!(c.proxy_resolve["a.test"].port, Some(8443));
        assert_eq!(c.proxy_resolve["b.test"].port, Some(9));
        assert_eq!(c.proxy_upstream_ca, Some(PathBuf::from("/t/ca.pem")));
        // Never alone: it could otherwise make the daemon trust anything for the real hosts.
        let alone = |k: &str| (k == PROXY_UPSTREAM_CA_ENV).then(|| OsString::from("/t/ca.pem"));
        assert!(Config::from_args(["--data-dir".into(), "/d".into()], alone).is_err());
        let args = ["--data-dir", "/d", "--sandbox", "chroot"];
        assert!(Config::from_args(args.map(OsString::from), no_env).is_err());
    }

    #[test]
    fn an_unknown_argument_or_a_missing_value_is_refused() {
        assert!(Config::from_args(["--nope".into()], no_env).is_err());
        assert!(Config::from_args(["--data-dir".into()], no_env).is_err());
    }
}
