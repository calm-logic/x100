//! Checkpoints: the working files of a worktree, frozen in a commit under `refs/x100/`.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use x100_core::git::CommitId;

use super::{Git, name_ok, out, run, run_raw};
use crate::error::{Error, Result, io};

static SERIAL: AtomicU64 = AtomicU64::new(0);

/// A private copy of a worktree's index. Git operations that would change the real index change
/// this one instead, and it is deleted on drop.
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    /// `from_real` starts from the worktree's current index, which keeps git's record of file
    /// timestamps so unchanged files are not read again.
    fn new(worktree: &Path, from_real: bool) -> Result<Self> {
        let gitdir = PathBuf::from(out(
            worktree,
            ["rev-parse", "--path-format=absolute", "--git-dir"],
            &[],
        )?);
        let path = gitdir.join(format!(
            "x100-index-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let real = gitdir.join("index");
        if from_real && real.exists() {
            fs::copy(&real, &path).map_err(io(&path))?;
        }
        Ok(Self { path })
    }

    fn env(&self) -> [(&'static str, &OsStr); 1] {
        [("GIT_INDEX_FILE", self.path.as_os_str())]
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

/// What changed in a worktree since a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff {
    pub files: Vec<Change>,
    /// A unified diff. Binary files appear as a line saying they differ.
    pub patch: String,
}

impl Git {
    /// Commits everything in `worktree` (staged, unstaged and untracked changes, deletions, modes)
    /// to `refs/x100/checkpoints/<name>` and returns the commit. The commit's parent is the
    /// worktree's current commit, so the objects stay reachable and a diff against the branch is
    /// meaningful. The branch, the real index and the working files are not touched. Ignored files
    /// are left out, as git leaves them out.
    pub fn checkpoint(&self, worktree: &Path, name: &str) -> Result<CommitId> {
        ref_name_ok(name)?;
        let tree = working_tree(worktree)?;
        let commit = out(
            worktree,
            ["commit-tree", &tree, "-p", "HEAD", "-m", "x100 checkpoint"],
            &[],
        )?;
        let full = format!("refs/x100/checkpoints/{name}");
        run(worktree, ["update-ref", &full, &commit], &[])?;
        Ok(CommitId(commit))
    }

    /// Makes the working files of `worktree` exactly those of `checkpoint`: files it lacks are
    /// removed (untracked ones included), the rest are written with their modes. Ignored files are
    /// left alone. The branch does not move; the index is reset to the branch's commit, because a
    /// checkpoint does not record what was staged.
    pub fn restore(&self, worktree: &Path, checkpoint: &CommitId) -> Result<()> {
        name_ok(&checkpoint.0)?;
        let idx = TempIndex::new(worktree, false)?;
        let env = idx.env();
        run(worktree, ["read-tree", &checkpoint.0], &env)?;
        let others = run(
            worktree,
            ["ls-files", "-z", "--others", "--exclude-standard"],
            &env,
        )?;
        for rel in others.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let rel = String::from_utf8_lossy(rel).into_owned();
            let path = worktree.join(&rel);
            fs::remove_file(&path).map_err(io(&path))?;
            prune_empty_dirs(worktree, path.parent());
        }
        run(worktree, ["checkout-index", "-a", "-f"], &env)?;
        run(worktree, ["reset", "-q"], &[])?;
        Ok(())
    }

    /// What `worktree` has changed since `checkpoint`.
    pub fn diff_since(&self, worktree: &Path, checkpoint: &CommitId) -> Result<Diff> {
        name_ok(&checkpoint.0)?;
        let tree = working_tree(worktree)?;
        let names = run(
            worktree,
            [
                "diff-tree",
                "-r",
                "-z",
                "--no-renames",
                "--name-status",
                &checkpoint.0,
                &tree,
            ],
            &[],
        )?;
        let mut files = Vec::new();
        let mut parts = names.stdout.split(|b| *b == 0).filter(|p| !p.is_empty());
        while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
            let kind = match status {
                b"A" => ChangeKind::Added,
                b"D" => ChangeKind::Deleted,
                _ => ChangeKind::Modified,
            };
            files.push(Change {
                path: String::from_utf8_lossy(path).into_owned(),
                kind,
            });
        }
        let patch = run_raw(
            worktree,
            [
                "diff-tree",
                "-r",
                "-p",
                "--no-renames",
                "--no-color",
                &checkpoint.0,
                &tree,
            ],
            &[],
        )?;
        Ok(Diff {
            files,
            patch: String::from_utf8_lossy(&patch.stdout).into_owned(),
        })
    }
}

/// The tree of everything in the worktree, written without touching its index.
fn working_tree(worktree: &Path) -> Result<String> {
    let idx = TempIndex::new(worktree, true)?;
    let env = idx.env();
    run(worktree, ["add", "-A"], &env)?;
    out(worktree, ["write-tree"], &env)
}

fn prune_empty_dirs(root: &Path, mut dir: Option<&Path>) {
    while let Some(d) = dir {
        if d == root || fs::remove_dir(d).is_err() {
            return;
        }
        dir = d.parent();
    }
}

fn ref_name_ok(name: &str) -> Result<()> {
    let fine = !name.is_empty()
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("..")
        && !name.contains("//")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.'))
        && !name
            .split('/')
            .any(|p| p.starts_with('.') || p.ends_with(".lock"));
    if fine {
        Ok(())
    } else {
        Err(Error::BadName(name.to_owned()))
    }
}
