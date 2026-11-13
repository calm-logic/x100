//! One git repository: `main`, which is what agents load from and which is checked out for people
//! to read, and branches named `trial/...` for changes on trial, made without touching the
//! checkout.
//!
//! A change to `main` is written into the checkout and committed with `git commit --only` on the
//! paths it names, so what the builder has staged or edited elsewhere is left alone. A change to a
//! branch is built with git's plumbing on a private index and moves only that branch's ref. Both
//! are read back from commits, never from the checkout, so a half-edited file does not reach an
//! agent.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use x100_core::git::CommitId;

use crate::error::{Error, Result, io};
use crate::git::{out, raw, run};

const MAIN: &str = "refs/heads/main";

/// The daemon is the only writer in v1, so one at a time is enough, and it keeps two writes to one
/// repository from meeting. A storage engine with several writers replaces this.
static WRITING: Mutex<()> = Mutex::new(());

/// Where a change goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The main branch, which agents load from.
    Main,
    /// A branch for a change on trial. It must start `trial/`.
    Branch(String),
}

/// A file to write, or `None` to remove it, by its path in the repository.
pub(crate) type Change = (String, Option<String>);

#[derive(Debug, Clone)]
pub(crate) struct Repo {
    dir: PathBuf,
}

impl Repo {
    pub fn at(dir: PathBuf) -> Repo {
        Repo { dir }
    }

    pub fn exists(&self) -> bool {
        self.dir.join(".git").exists()
    }

    /// Starts the repository if it is not there, with an empty first commit for branches to start
    /// from. Returns whether it did.
    pub fn ensure(&self) -> Result<bool> {
        if self.exists() {
            return Ok(false);
        }
        fs::create_dir_all(&self.dir).map_err(io(&self.dir))?;
        run(&self.dir, &["init", "-q", "-b", "main"], &[], None)?;
        run(
            &self.dir,
            &["commit", "-q", "--allow-empty", "-m", "x100: start"],
            &[],
            None,
        )?;
        Ok(true)
    }

    pub fn main_tip(&self) -> Result<CommitId> {
        out(
            &self.dir,
            &["rev-parse", "--verify", &format!("{MAIN}^{{commit}}")],
        )
        .map(CommitId)
    }

    fn branch_ref(branch: &str) -> Result<String> {
        let ok = branch.strip_prefix("trial/").is_some_and(|rest| {
            !rest.is_empty()
                && !rest.contains("..")
                && !rest.ends_with(['/', '.'])
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
        });
        if !ok {
            return Err(Error::Invalid(format!(
                "`{branch}` is not a trial branch: it starts `trial/`, then letters, digits, `.`, `_`, `-` or `/`"
            )));
        }
        Ok(format!("refs/heads/{branch}"))
    }

    fn rev_of(&self, target: &Target) -> Result<String> {
        match target {
            Target::Main => Ok(MAIN.to_owned()),
            Target::Branch(b) => Self::branch_ref(b),
        }
    }

    /// The commit a branch is at, if it exists.
    pub fn tip(&self, target: &Target) -> Result<Option<CommitId>> {
        let rev = self.rev_of(target)?;
        let o = raw(
            &self.dir,
            &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
            &[],
            None,
        )?;
        Ok(o.status
            .success()
            .then(|| CommitId(String::from_utf8_lossy(&o.stdout).trim().to_owned())))
    }

    /// `target`, or main where this repository has no such branch: a change on trial lives in
    /// one repository, and the others are read as they are.
    pub fn or_main(&self, target: &Target) -> Result<Target> {
        Ok(match target {
            Target::Branch(_) if self.tip(target)?.is_none() => Target::Main,
            other => other.clone(),
        })
    }

    /// The names of the trial branches.
    pub fn branches(&self) -> Result<Vec<String>> {
        let text = out(
            &self.dir,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/trial/",
            ],
        )?;
        Ok(text.lines().map(str::to_owned).collect())
    }

    /// Every file under `prefix` at `target`, by path. Empty if `target` is a branch that is not
    /// there.
    pub fn files(&self, target: &Target, prefix: &str) -> Result<BTreeMap<String, String>> {
        if self.tip(target)?.is_none() {
            return Ok(BTreeMap::new());
        }
        let rev = self.rev_of(target)?;
        let mut args = vec!["ls-tree", "-r", "-z", "--name-only", &rev];
        if !prefix.is_empty() {
            args.extend(["--", prefix]);
        }
        let listed = run(&self.dir, &args, &[], None)?;
        let paths: Vec<String> = listed
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        self.blobs(&rev, &paths)
    }

    /// The commit main and a branch last had in common, which is where the change on the branch
    /// began.
    pub fn base(&self, branch: &str) -> Result<CommitId> {
        let rev = Self::branch_ref(branch)?;
        out(&self.dir, &["merge-base", MAIN, &rev]).map(CommitId)
    }

    /// Every file under `prefix` at a commit, by path.
    pub fn files_at(&self, commit: &CommitId, prefix: &str) -> Result<BTreeMap<String, String>> {
        let rev = commit.0.clone();
        let listed = run(
            &self.dir,
            &["ls-tree", "-r", "-z", "--name-only", &rev, "--", prefix],
            &[],
            None,
        )?;
        let paths: Vec<String> = listed
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        self.blobs(&rev, &paths)
    }

    /// One file at `target`, if it is there.
    pub fn file(&self, target: &Target, path: &str) -> Result<Option<String>> {
        Ok(self
            .blobs(&self.rev_of(target)?, &[path.to_owned()])?
            .remove(path))
    }

    /// The named files at `rev`, in one `cat-file --batch`, which is why a thousand notes are one
    /// process and not a thousand. A path that is not there is left out.
    fn blobs(&self, rev: &str, paths: &[String]) -> Result<BTreeMap<String, String>> {
        let mut found = BTreeMap::new();
        if paths.is_empty() {
            return Ok(found);
        }
        let input: String = paths.iter().map(|p| format!("{rev}:{p}\n")).collect();
        let o = run(
            &self.dir,
            &["cat-file", "--batch"],
            &[],
            Some(input.as_bytes()),
        )?;
        let mut rest = o.stdout.as_slice();
        for path in paths {
            let Some(eol) = rest.iter().position(|b| *b == b'\n') else {
                break;
            };
            let header = String::from_utf8_lossy(&rest[..eol]).into_owned();
            rest = &rest[eol + 1..];
            let mut words = header.split(' ');
            let (_oid, kind, size) = (words.next(), words.next(), words.next());
            if kind != Some("blob") {
                continue;
            }
            let size: usize = size.and_then(|s| s.parse().ok()).unwrap_or(0);
            let end = size.min(rest.len());
            found.insert(
                path.clone(),
                String::from_utf8_lossy(&rest[..end]).into_owned(),
            );
            rest = rest.get(end + 1..).unwrap_or(&[]);
        }
        Ok(found)
    }

    /// Commits `changes` to `target`, with `message`. `None` if they change nothing.
    pub fn commit(
        &self,
        target: &Target,
        changes: &[Change],
        message: &str,
    ) -> Result<Option<CommitId>> {
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        match target {
            Target::Main => self.commit_main(changes, message),
            Target::Branch(b) => self.commit_branch(b, changes, message),
        }
    }

    fn commit_main(&self, changes: &[Change], message: &str) -> Result<Option<CommitId>> {
        let head = out(&self.dir, &["symbolic-ref", "HEAD"])?;
        if head != MAIN {
            return Err(Error::Invalid(format!(
                "{} has {head} checked out, not main: put main back before x100 writes here",
                self.dir.display()
            )));
        }
        let mut paths: Vec<&str> = Vec::new();
        for (path, content) in changes {
            let file = self.dir.join(path);
            match content {
                Some(text) => {
                    if let Some(parent) = file.parent() {
                        fs::create_dir_all(parent).map_err(io(parent))?;
                    }
                    fs::write(&file, text).map_err(io(&file))?;
                    paths.push(path);
                }
                None => match fs::remove_file(&file) {
                    Ok(()) => paths.push(path),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io(&file)(e)),
                },
            }
        }
        // A removed file is added only if git has it, since adding a path git has never seen is
        // an error.
        let mut ls = vec!["ls-files", "-z", "--"];
        ls.extend(paths.iter().copied());
        let tracked = run(&self.dir, &ls, &[], None)?.stdout;
        let tracked: Vec<String> = tracked
            .split(|b| *b == 0)
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        paths.retain(|p| {
            changes.iter().any(|(c, text)| c == p && text.is_some())
                || tracked.iter().any(|t| t == p)
        });
        if paths.is_empty() {
            return Ok(None);
        }
        let mut add = vec!["add", "-A", "--"];
        add.extend(paths.iter().copied());
        run(&self.dir, &add, &[], None)?;
        let mut diff = vec!["diff", "--cached", "--quiet", "--"];
        diff.extend(paths.iter().copied());
        if raw(&self.dir, &diff, &[], None)?.status.success() {
            return Ok(None);
        }
        let mut commit = vec!["commit", "-q", "--only", "-m", message, "--"];
        commit.extend(paths.iter().copied());
        run(&self.dir, &commit, &[], None)?;
        self.main_tip().map(Some)
    }

    fn commit_branch(
        &self,
        branch: &str,
        changes: &[Change],
        message: &str,
    ) -> Result<Option<CommitId>> {
        static N: AtomicU64 = AtomicU64::new(0);
        let target = Target::Branch(branch.to_owned());
        let rev = Self::branch_ref(branch)?;
        let base = self.tip(&target)?.map_or_else(|| self.main_tip(), Ok)?;
        let git_dir = PathBuf::from(out(
            &self.dir,
            &["rev-parse", "--path-format=absolute", "--git-dir"],
        )?);
        let index = git_dir.join(format!(
            "x100-index-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let made = self.build(&index, &base, changes, message);
        let _ = fs::remove_file(&index);
        let commit = made?;
        if commit == base {
            // A branch that has not been made yet still gets its ref, at main.
            if self.tip(&target)?.is_none() {
                run(&self.dir, &["update-ref", &rev, &commit.0], &[], None)?;
            }
            return Ok(None);
        }
        run(&self.dir, &["update-ref", &rev, &commit.0], &[], None)?;
        Ok(Some(commit))
    }

    /// The commit of `changes` on top of `base`, made on a private index.
    fn build(
        &self,
        index: &Path,
        base: &CommitId,
        changes: &[Change],
        message: &str,
    ) -> Result<CommitId> {
        let env: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", index.as_os_str())];
        run(&self.dir, &["read-tree", &base.0], &env, None)?;
        for (path, content) in changes {
            match content {
                Some(text) => {
                    let blob = run(
                        &self.dir,
                        &["hash-object", "-w", "--stdin"],
                        &[],
                        Some(text.as_bytes()),
                    )?;
                    let oid = String::from_utf8_lossy(&blob.stdout).trim().to_owned();
                    let info = format!("100644,{oid},{path}");
                    run(
                        &self.dir,
                        &["update-index", "--add", "--cacheinfo", &info],
                        &env,
                        None,
                    )?;
                }
                None => {
                    run(
                        &self.dir,
                        &["update-index", "--force-remove", "--", path],
                        &env,
                        None,
                    )?;
                }
            }
        }
        let tree = run(&self.dir, &["write-tree"], &env, None)?;
        let tree = String::from_utf8_lossy(&tree.stdout).trim().to_owned();
        let before = out(&self.dir, &["rev-parse", &format!("{}^{{tree}}", base.0)])?;
        if tree == before {
            return Ok(base.clone());
        }
        let commit = run(
            &self.dir,
            &["commit-tree", &tree, "-p", &base.0, "-m", message],
            &[],
            None,
        )?;
        Ok(CommitId(
            String::from_utf8_lossy(&commit.stdout).trim().to_owned(),
        ))
    }

    /// Merges a trial branch into main and deletes it. A merge that conflicts is undone and the
    /// branch kept.
    pub fn merge(&self, branch: &str) -> Result<CommitId> {
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let rev = Self::branch_ref(branch)?;
        if self.tip(&Target::Branch(branch.to_owned()))?.is_none() {
            return Err(Error::NotFound(format!("branch {branch}")));
        }
        let message = format!("x100: merge {branch}");
        let o = raw(
            &self.dir,
            &["merge", "-q", "--no-edit", "-m", &message, &rev],
            &[],
            None,
        )?;
        if !o.status.success() {
            let why = format!(
                "{} {}",
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            let _ = raw(&self.dir, &["merge", "--abort"], &[], None);
            return Err(Error::Conflict {
                branch: branch.to_owned(),
                why: why.trim().to_owned(),
            });
        }
        run(&self.dir, &["update-ref", "-d", &rev], &[], None)?;
        self.main_tip()
    }

    /// Deletes a trial branch, which is how a change on trial is dropped.
    pub fn discard(&self, branch: &str) -> Result<()> {
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let rev = Self::branch_ref(branch)?;
        if self.tip(&Target::Branch(branch.to_owned()))?.is_none() {
            return Err(Error::NotFound(format!("branch {branch}")));
        }
        run(&self.dir, &["update-ref", "-d", &rev], &[], None)?;
        Ok(())
    }
}
