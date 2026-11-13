//! Git for one project directory, by running the `git` command line.
//!
//! Why the command line and not `git2` or `gix`: the checkpoint is "commit the working files,
//! untracked ones included, without touching the branch or the index", which git does with a
//! throwaway `GIT_INDEX_FILE`, `add -A`, `write-tree`, `commit-tree` and `update-ref`. Those
//! plumbing commands are stable and behave the same on Linux and macOS. `git2` can build the same
//! tree but needs libgit2's index handling worked around, and `gix` cannot yet add a linked
//! worktree or write an index the way this needs. x100's agents run `git` in their sandboxes
//! anyway, so it is already a requirement.
//!
//! Every command runs with hooks off, no prompts, and a fixed identity, so what the user has
//! configured cannot change what x100 does, and x100 never asks for a password.

mod branch;
mod snapshot;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use x100_core::git::CommitId;
use x100_core::id::AgentId;

use crate::error::{Error, Result, io};

pub use branch::MergeOutcome;
pub use snapshot::{Change, ChangeKind, Diff};

/// A project's code directory: its own repository, and `.x100/` beside it with the agents' worktrees
/// and what x100 writes there for people, kept out of the repository.
#[derive(Debug, Clone)]
pub struct Git {
    project: PathBuf,
}

impl Git {
    pub fn new(project: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
        }
    }

    pub fn project_dir(&self) -> &Path {
        &self.project
    }

    /// Where an agent's worktree is.
    pub fn worktree_path(&self, agent: AgentId) -> PathBuf {
        self.project
            .join(".x100")
            .join("worktrees")
            .join(agent.to_string())
    }

    /// Makes the directory ready for x100: starts a repository if there is none, keeps `.x100/`
    /// out of it through `.git/info/exclude` (the project's own `.gitignore` is never edited), and
    /// makes a first commit of the files already there if the repository has none, since a
    /// worktree needs a commit to branch from. Safe to run again.
    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.project).map_err(io(&self.project))?;
        if !self.project.join(".git").exists() {
            run(&self.project, ["init", "-q", "-b", "main"], &[])?;
        }
        self.exclude_x100()?;
        if run(&self.project, ["rev-parse", "--verify", "-q", "HEAD"], &[]).is_err() {
            run(&self.project, ["add", "-A"], &[])?;
            run(
                &self.project,
                [
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "x100: the project as it was",
                ],
                &[],
            )?;
        }
        Ok(())
    }

    fn exclude_x100(&self) -> Result<()> {
        let common = out(
            &self.project,
            ["rev-parse", "--path-format=absolute", "--git-common-dir"],
            &[],
        )?;
        let file = PathBuf::from(common).join("info").join("exclude");
        let current = fs::read_to_string(&file).unwrap_or_default();
        if current.lines().any(|l| l.trim() == ".x100/") {
            return Ok(());
        }
        let parent = file.parent().expect("the exclude file has a directory");
        fs::create_dir_all(parent).map_err(io(parent))?;
        let sep = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        fs::write(&file, format!("{current}{sep}.x100/\n")).map_err(io(&file))
    }

    /// The branch the directory has checked out.
    pub fn current_branch(&self, dir: &Path) -> Result<String> {
        out(dir, ["symbolic-ref", "--short", "HEAD"], &[])
    }

    /// The commit a branch or ref points at.
    pub fn resolve(&self, rev: &str) -> Result<CommitId> {
        name_ok(rev)?;
        let spec = format!("{rev}^{{commit}}");
        out(&self.project, ["rev-parse", "--verify", &spec], &[]).map(CommitId)
    }

    /// The newest commit both revisions have, as the project's checkout sees them.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<CommitId> {
        name_ok(a)?;
        name_ok(b)?;
        out(&self.project, ["merge-base", a, b], &[]).map(CommitId)
    }
}

/// Refuses a name that git could read as an option.
fn name_ok(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.contains('\0') {
        return Err(Error::BadName(name.to_owned()));
    }
    Ok(())
}

const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "x100"),
    ("GIT_AUTHOR_EMAIL", "x100@localhost"),
    ("GIT_COMMITTER_NAME", "x100"),
    ("GIT_COMMITTER_EMAIL", "x100@localhost"),
];

fn command<I, S>(dir: &Path, args: I, env: &[(&str, &OsStr)]) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
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

/// Runs git and returns its output, whatever its exit status.
fn run_raw<I, S>(dir: &Path, args: I, env: &[(&str, &OsStr)]) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = command(dir, args, env);
    c.output().map_err(io(dir))
}

fn describe<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs git and fails on a non-zero exit, with git's own message.
fn run<I, S>(dir: &Path, args: I, env: &[(&str, &OsStr)]) -> Result<Output>
where
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<OsStr>,
{
    let o = run_raw(dir, args.clone(), env)?;
    if o.status.success() {
        Ok(o)
    } else {
        Err(Error::Git {
            args: describe(args),
            stderr: String::from_utf8_lossy(&o.stderr).trim().to_owned(),
        })
    }
}

/// Runs git and returns its trimmed standard output as text.
fn out<I, S>(dir: &Path, args: I, env: &[(&str, &OsStr)]) -> Result<String>
where
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<OsStr>,
{
    let o = run(dir, args, env)?;
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
}
