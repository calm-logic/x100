//! Worktrees and merges.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use x100_core::git::CommitId;
use x100_core::id::AgentId;

use super::{Git, name_ok, out, run, run_raw};
use crate::error::{Error, Result, io};

/// How a merge ended. A conflict is a result, not an error: the merge is undone and the caller
/// decides what to do, which for a task is to send it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeOutcome {
    /// The target already had everything.
    UpToDate,
    /// The target branch now points at `commit`, a merge or a fast-forward.
    Merged { commit: CommitId },
    /// Nothing changed. `files` conflict and `detail` is git's account.
    Conflict { files: Vec<String>, detail: String },
}

impl Git {
    /// Creates `.x100/worktrees/<agent>/` on a new branch `branch` starting at `from`.
    pub fn create_worktree(&self, agent: AgentId, branch: &str, from: &str) -> Result<PathBuf> {
        name_ok(branch)?;
        name_ok(from)?;
        let path = self.worktree_path(agent);
        if path.exists() {
            return Err(Error::WorktreeExists(path));
        }
        let parent = path.parent().expect("a worktree path has a parent");
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        let p = path.to_string_lossy();
        run(
            &self.project,
            ["worktree", "add", "-q", "-b", branch, &p, from],
            &[],
        )?;
        Ok(path)
    }

    /// Removes an agent's worktree, with whatever is uncommitted in it. The branch stays.
    pub fn remove_worktree(&self, agent: AgentId) -> Result<()> {
        let path = self.worktree_path(agent);
        let p = path.to_string_lossy();
        run(&self.project, ["worktree", "remove", "--force", &p], &[]).map(drop)
    }

    /// Commits everything in `worktree` (staged, unstaged and untracked changes, deletions,
    /// modes) to the branch it has checked out, so a merge of that branch carries it. Returns the
    /// new commit, or `None` when there was nothing to commit. Ignored files are left out, as git
    /// leaves them out.
    pub fn commit(&self, worktree: &Path, message: &str) -> Result<Option<CommitId>> {
        run(worktree, ["add", "-A"], &[])?;
        let staged = run_raw(worktree, ["diff", "--cached", "--quiet"], &[])?;
        match staged.status.code() {
            Some(0) => return Ok(None),
            Some(1) => {}
            _ => {
                return Err(Error::Git {
                    args: "diff --cached --quiet".into(),
                    stderr: String::from_utf8_lossy(&staged.stderr).trim().to_owned(),
                });
            }
        }
        run(worktree, ["commit", "-q", "-m", message], &[])?;
        out(worktree, ["rev-parse", "HEAD"], &[]).map(|c| Some(CommitId(c)))
    }

    /// Merges branch `from` into whatever branch `into` (a worktree, or the project directory) has
    /// checked out. A merge needs a checked-out target, and x100 keeps every branch in one.
    pub fn merge(&self, into: &Path, from: &str) -> Result<MergeOutcome> {
        name_ok(from)?;
        let before = out(into, ["rev-parse", "HEAD"], &[])?;
        let message = format!("x100: merge {from}");
        let merge = run_raw(into, ["merge", "--no-edit", "-m", &message, from], &[])?;
        if merge.status.success() {
            let after = out(into, ["rev-parse", "HEAD"], &[])?;
            return Ok(if after == before {
                MergeOutcome::UpToDate
            } else {
                MergeOutcome::Merged {
                    commit: CommitId(after),
                }
            });
        }
        let unmerged = run(into, ["diff", "--name-only", "--diff-filter=U", "-z"], &[])?;
        let files: Vec<String> = unmerged
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        let detail = String::from_utf8_lossy(&merge.stdout).trim().to_owned();
        if files.is_empty() {
            // Refused before it began, for instance over uncommitted changes the merge would
            // overwrite. Nothing to undo.
            return Err(Error::Git {
                args: format!("merge {from}"),
                stderr: format!("{detail}\n{}", String::from_utf8_lossy(&merge.stderr)),
            });
        }
        run(into, ["merge", "--abort"], &[])?;
        Ok(MergeOutcome::Conflict { files, detail })
    }
}
