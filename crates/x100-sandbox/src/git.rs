//! What of the project's git directory a sandbox needs for git to work on a linked worktree.

use std::path::{Path, PathBuf};

use crate::{Error, Mount, io};

/// The paths that let git inside the sandbox read and commit on `worktree`'s branch.
///
/// A linked worktree keeps its index and `HEAD` in an administrative directory of the project's
/// git directory, and its objects and refs in the project's git directory itself. So: the git
/// directory read-only (its config and hooks above all, which the daemon's own git runs would
/// otherwise read); the objects, the worktree's administrative directory, and the directory that
/// holds the branch's ref and its log read-write.
///
/// Known limits: every committed object of the project is readable, other agents' committed
/// work included, and so is every branch beside this one in its ref directory (all agents'
/// branches while they are named `x100/<agent>`), which is writable. A worktree that is a whole
/// repository of its own needs nothing.
pub fn git_paths(worktree: &Path) -> Result<Vec<Mount>, Error> {
    let marker = worktree.join(".git");
    if marker.is_dir() {
        return Ok(Vec::new());
    }
    let read = |p: &Path| {
        std::fs::read_to_string(p)
            .map(|s| s.trim().to_owned())
            .map_err(io(format!("reading {}", p.display())))
    };
    let text = read(&marker)?;
    let admin = text
        .strip_prefix("gitdir:")
        .map(|s| absolute(worktree, s.trim()))
        .ok_or_else(|| Error::Spec(format!("{} is not a worktree link", marker.display())))?;
    let common = match read(&admin.join("commondir")) {
        Ok(rel) => absolute(&admin, &rel),
        Err(_) => admin.clone(),
    };
    let head = read(&admin.join("HEAD"))?;
    let branch_dir = head
        .strip_prefix("ref: ")
        .and_then(|r| Path::new(r).parent().map(Path::to_path_buf))
        .ok_or_else(|| Error::Spec(format!("{} is not on a branch", worktree.display())))?;
    let refs = common.join(&branch_dir);
    let logs = common.join("logs").join(&branch_dir);
    // Git makes the log's directory on first use, which it could not do under the read-only
    // git directory.
    for d in [&refs, &logs] {
        std::fs::create_dir_all(d).map_err(io(format!("making {}", d.display())))?;
    }
    let mut paths = vec![
        Mount::read_only(&common),
        Mount::read_write(common.join("objects")),
        Mount::read_write(refs),
        Mount::read_write(logs),
    ];
    if admin != common {
        paths.push(Mount::read_write(&admin));
    }
    Ok(paths)
}

/// Resolved, so that `commondir`'s `../..` does not make a parent sort after its children.
fn absolute(base: &Path, p: &str) -> PathBuf {
    let joined = base.join(p);
    joined.canonicalize().unwrap_or(joined)
}
