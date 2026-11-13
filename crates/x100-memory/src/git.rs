//! The `git` command line, as `x100-store` runs it: hooks off, no prompts, a fixed identity, so
//! nothing the builder has configured changes what x100 commits. It is a second copy of those
//! few lines because this crate must stand alone for the storage engine that replaces it.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::error::{Error, Result, io};

const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "x100"),
    ("GIT_AUTHOR_EMAIL", "x100@localhost"),
    ("GIT_COMMITTER_NAME", "x100"),
    ("GIT_COMMITTER_EMAIL", "x100@localhost"),
];

fn command(dir: &Path, args: &[&OsStr], env: &[(&str, &OsStr)]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("GIT_MERGE_AUTOEDIT", "no");
    for (k, v) in IDENTITY {
        c.env(k, v);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

/// Runs git with `stdin` if any, and returns its output whatever its exit status.
pub(crate) fn raw(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &OsStr)],
    stdin: Option<&[u8]>,
) -> Result<Output> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let mut c = command(dir, &args, env);
    let Some(input) = stdin else {
        return c.output().map_err(io(dir));
    };
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().map_err(io(dir))?;
    let mut pipe = child.stdin.take().expect("stdin was piped");
    // Written beside the reading, since git may fill its output before it has read all its input.
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&input);
    });
    let output = child.wait_with_output().map_err(io(dir));
    let _ = writer.join();
    output
}

/// Runs git and fails on a non-zero exit, with git's own message.
pub(crate) fn run(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &OsStr)],
    stdin: Option<&[u8]>,
) -> Result<Output> {
    let o = raw(dir, args, env, stdin)?;
    if o.status.success() {
        Ok(o)
    } else {
        Err(Error::Git {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&o.stderr).trim().to_owned(),
        })
    }
}

/// Runs git and returns its trimmed standard output as text.
pub(crate) fn out(dir: &Path, args: &[&str]) -> Result<String> {
    let o = run(dir, args, &[], None)?;
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
}
