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
pub const SCRIPTS_ENV: &str = "X100_FAKE_SCRIPTS";
pub const FAKE_PROGRAM_ENV: &str = "X100_FAKE_HARNESS";

#[derive(Clone)]
pub struct Config {
    /// Holds the daemon's log, the socket and the list of driver processes.
    pub data_dir: PathBuf,
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
}

impl Config {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            scripts_dir: None,
            fake_program: None,
            max_line: MAX_LINE,
            queue_capacity: QUEUE_CAPACITY,
            lag_after: LAG_AFTER,
            clock: Arc::new(SystemClock),
            fake_clock: None,
        }
    }

    pub fn socket_path(&self) -> PathBuf {
        self.data_dir.join("x100d.sock")
    }

    /// From `--data-dir`, `--scripts-dir` and `--fake-harness` arguments, each of which falls back to
    /// its environment variable, and `--fake-clock`. The data directory then falls back to the
    /// user's data directory.
    pub fn from_args(
        args: impl IntoIterator<Item = OsString>,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, String> {
        let (mut data, mut scripts, mut fake, mut clock) = (None, None, None, None);
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let slot = match arg.to_str() {
                Some("--data-dir") => &mut data,
                Some("--scripts-dir") => &mut scripts,
                Some("--fake-harness") => &mut fake,
                Some("--fake-clock") => &mut clock,
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
        config.scripts_dir = scripts
            .map(PathBuf::from)
            .or_else(|| env(SCRIPTS_ENV).map(PathBuf::from));
        config.fake_program = fake
            .map(PathBuf::from)
            .or_else(|| env(FAKE_PROGRAM_ENV).map(PathBuf::from));
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
    fn an_unknown_argument_or_a_missing_value_is_refused() {
        assert!(Config::from_args(["--nope".into()], no_env).is_err());
        assert!(Config::from_args(["--data-dir".into()], no_env).is_err());
    }
}
