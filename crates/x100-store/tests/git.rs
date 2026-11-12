use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

use x100_core::id::AgentId;
use x100_store::git::{ChangeKind, MergeOutcome};
use x100_store::{Error, Git};

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Every file under `dir` outside `.git`, by relative path, with its mode and bytes (or link
/// target), for comparing two states of a working tree.
fn files_of(dir: &Path) -> BTreeMap<String, (u32, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u32, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if rel == ".git" {
                continue;
            }
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(root, &path, out);
            } else if meta.is_symlink() {
                let target = fs::read_link(&path).unwrap();
                out.insert(
                    rel,
                    (0o120000, target.to_string_lossy().into_owned().into_bytes()),
                );
            } else {
                // Git records only the executable bit; the rest of a mode is the umask's.
                let mode = if meta.permissions().mode() & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                };
                out.insert(rel, (mode, fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn project_with_history() -> (tempfile::TempDir, Git) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    write(&p.join("README.md"), "readme\n");
    write(&p.join("src/main.rs"), "fn main() {}\n");
    write(&p.join("src/old.rs"), "old\n");
    write(&p.join("build.sh"), "#!/bin/sh\necho hi\n");
    chmod(&p.join("build.sh"), 0o755);
    write(&p.join("keep/inside.txt"), "x\n");
    let git = Git::new(p);
    git.prepare().unwrap();
    (dir, git)
}

#[test]
fn preparing_a_directory_starts_a_repository_and_hides_x100_without_touching_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    write(&p.join("a.txt"), "a\n");
    write(&p.join(".gitignore"), "target/\n");
    let git = Git::new(p);
    git.prepare().unwrap();
    git.prepare().unwrap();

    assert_eq!(git.current_branch(p).unwrap(), "main");
    assert!(sh(p, &["ls-files"]).contains("a.txt"));
    assert_eq!(
        fs::read_to_string(p.join(".gitignore")).unwrap(),
        "target/\n"
    );

    write(&p.join(".x100/store.sqlite"), "db");
    write(&p.join(".x100/worktrees/x/f"), "f");
    assert_eq!(sh(p, &["status", "--porcelain"]), "");
    let exclude = fs::read_to_string(p.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches(".x100/").count(), 1);
}

#[test]
fn an_existing_repository_keeps_its_history_and_gets_no_extra_commit() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "-q", "-b", "trunk"]);
    write(&p.join("f"), "1\n");
    sh(p, &["add", "-A"]);
    sh(p, &["commit", "-q", "-m", "mine"]);
    let before = sh(p, &["rev-parse", "HEAD"]);
    write(&p.join("dirty"), "uncommitted\n");

    Git::new(p).prepare().unwrap();
    assert_eq!(sh(p, &["rev-parse", "HEAD"]), before);
    assert_eq!(sh(p, &["rev-list", "--count", "HEAD"]).trim(), "1");
    assert!(sh(p, &["status", "--porcelain"]).contains("dirty"));
}

#[test]
fn a_worktree_gets_its_own_branch_under_x100_worktrees() {
    let (dir, git) = project_with_history();
    let agent = AgentId::from_u128(7);
    let wt = git.create_worktree(agent, "x100/agent-7", "main").unwrap();
    assert_eq!(
        wt,
        dir.path().join(".x100/worktrees").join(agent.to_string())
    );
    assert_eq!(git.current_branch(&wt).unwrap(), "x100/agent-7");
    let tree = files_of(&wt);
    assert_eq!(tree["build.sh"].0, 0o755);
    assert_eq!(tree["src/main.rs"].1, b"fn main() {}\n");
    assert!(matches!(
        git.create_worktree(agent, "x100/other", "main"),
        Err(Error::WorktreeExists(_))
    ));
    assert!(matches!(
        git.create_worktree(AgentId::from_u128(8), "--bad", "main"),
        Err(Error::BadName(_))
    ));
    git.remove_worktree(agent).unwrap();
    assert!(!wt.exists());
}

/// The state a checkpoint must not disturb.
fn git_state(wt: &Path) -> (String, String, String) {
    (
        sh(wt, &["rev-parse", "HEAD"]),
        sh(wt, &["ls-files", "-s"]),
        sh(wt, &["status", "--porcelain=v2", "-uall"]),
    )
}

#[test]
fn a_checkpoint_restores_the_same_files_and_modes_and_disturbs_neither_branch_nor_index() {
    let (_dir, git) = project_with_history();
    let wt = git
        .create_worktree(AgentId::from_u128(1), "x100/a", "main")
        .unwrap();

    // Staged edit, then a further unstaged edit to the same file.
    write(&wt.join("README.md"), "staged\n");
    sh(&wt, &["add", "README.md"]);
    write(&wt.join("README.md"), "staged\nand unstaged\n");
    // Staged new file.
    write(&wt.join("staged_new.txt"), "new and staged\n");
    sh(&wt, &["add", "staged_new.txt"]);
    // Unstaged edit of a tracked file, made executable.
    write(&wt.join("src/main.rs"), "fn main() { println!(); }\n");
    chmod(&wt.join("src/main.rs"), 0o755);
    // Deleted tracked files, one deleted from the index too.
    fs::remove_file(wt.join("src/old.rs")).unwrap();
    sh(&wt, &["rm", "-q", "--cached", "keep/inside.txt"]);
    fs::remove_file(wt.join("keep/inside.txt")).unwrap();
    // Untracked: a plain file, an executable, a file in a new directory, a symlink, an empty file.
    write(&wt.join("untracked.txt"), "untracked\n");
    write(&wt.join("run.sh"), "#!/bin/sh\n");
    chmod(&wt.join("run.sh"), 0o755);
    write(&wt.join("brand/new/dir/file.txt"), "deep\n");
    symlink("README.md", wt.join("link")).unwrap();
    write(&wt.join("empty"), "");
    // Ignored files are not work.
    write(&wt.join(".gitignore"), "*.log\n");
    write(&wt.join("noise.log"), "noise");

    let before_state = git_state(&wt);
    let want = files_of(&wt);
    let head = sh(&wt, &["rev-parse", "HEAD"]).trim().to_owned();

    let cp = git.checkpoint(&wt, "agent-1/turn-1").unwrap();

    assert_eq!(
        git_state(&wt),
        before_state,
        "checkpointing moved something"
    );
    assert_eq!(files_of(&wt), want, "checkpointing changed a working file");
    assert_eq!(
        sh(&wt, &["rev-parse", "refs/x100/checkpoints/agent-1/turn-1"]).trim(),
        cp.0
    );
    assert_eq!(sh(&wt, &["rev-parse", &format!("{}^", cp.0)]).trim(), head);
    assert_eq!(
        git.resolve("refs/x100/checkpoints/agent-1/turn-1").unwrap(),
        cp
    );

    // Now wreck the worktree: change, delete and add files, and move the branch.
    write(&wt.join("README.md"), "wrecked\n");
    fs::remove_file(wt.join("run.sh")).unwrap();
    fs::remove_dir_all(wt.join("brand")).unwrap();
    write(&wt.join("extra/more.txt"), "extra\n");
    write(&wt.join("keep/inside.txt"), "back\n");
    sh(&wt, &["add", "-A"]);
    sh(&wt, &["commit", "-q", "-m", "later work"]);
    write(&wt.join("late.txt"), "late\n");

    git.restore(&wt, &cp).unwrap();

    let mut got = files_of(&wt);
    // The ignored file was never part of the checkpoint and is left where it was.
    assert_eq!(got.remove("noise.log").unwrap().1, b"noise");
    let mut want = want;
    want.remove("noise.log");
    assert_eq!(got, want);
    assert!(
        !wt.join("extra").exists(),
        "an emptied directory was left behind"
    );
    assert!(!wt.join("late.txt").exists());
}

#[test]
fn the_diff_since_a_checkpoint_lists_what_changed_after_it() {
    let (_dir, git) = project_with_history();
    let wt = git
        .create_worktree(AgentId::from_u128(1), "x100/a", "main")
        .unwrap();
    write(&wt.join("one.txt"), "1\n");
    let cp = git.checkpoint(&wt, "a/start").unwrap();
    assert!(git.diff_since(&wt, &cp).unwrap().files.is_empty());

    write(&wt.join("one.txt"), "1\n2\n");
    write(&wt.join("two.txt"), "two\n");
    fs::remove_file(wt.join("src/old.rs")).unwrap();
    let diff = git.diff_since(&wt, &cp).unwrap();
    let mut files: Vec<_> = diff
        .files
        .iter()
        .map(|c| (c.path.as_str(), c.kind))
        .collect();
    files.sort_by_key(|f| f.0);
    assert_eq!(
        files,
        [
            ("one.txt", ChangeKind::Modified),
            ("src/old.rs", ChangeKind::Deleted),
            ("two.txt", ChangeKind::Added),
        ]
    );
    assert!(diff.patch.contains("+2"));
    assert!(diff.patch.contains("+two"));
    assert!(diff.patch.contains("-old"));
}

#[test]
fn checkpoint_names_cannot_escape_the_hidden_namespace() {
    let (_dir, git) = project_with_history();
    for bad in ["../heads/main", "/abs", "a..b", "", "a/.hidden", "x.lock"] {
        assert!(
            matches!(
                git.checkpoint(git.project_dir(), bad),
                Err(Error::BadName(_))
            ),
            "{bad}"
        );
    }
}

#[test]
fn merging_reports_up_to_date_a_merge_and_a_conflict_as_values() {
    let (dir, git) = project_with_history();
    let main = dir.path();
    let a = git
        .create_worktree(AgentId::from_u128(1), "x100/a", "main")
        .unwrap();
    let b = git
        .create_worktree(AgentId::from_u128(2), "x100/b", "main")
        .unwrap();

    assert_eq!(git.merge(main, "x100/a").unwrap(), MergeOutcome::UpToDate);

    write(&a.join("a.txt"), "from a\n");
    sh(&a, &["add", "-A"]);
    sh(&a, &["commit", "-q", "-m", "a"]);
    write(&b.join("b.txt"), "from b\n");
    sh(&b, &["add", "-A"]);
    sh(&b, &["commit", "-q", "-m", "b"]);

    let MergeOutcome::Merged { commit } = git.merge(main, "x100/a").unwrap() else {
        panic!("expected a fast-forward");
    };
    assert_eq!(sh(main, &["rev-parse", "HEAD"]).trim(), commit.0);
    let MergeOutcome::Merged { .. } = git.merge(main, "x100/b").unwrap() else {
        panic!("expected a merge");
    };
    assert!(main.join("a.txt").exists() && main.join("b.txt").exists());

    // Both change the same line differently.
    write(&a.join("README.md"), "a's version\n");
    sh(&a, &["commit", "-q", "-am", "a edits"]);
    write(&b.join("README.md"), "b's version\n");
    sh(&b, &["commit", "-q", "-am", "b edits"]);
    git.merge(&a, "main").unwrap();
    let tip = sh(&a, &["rev-parse", "HEAD"]);
    match git.merge(&a, "x100/b").unwrap() {
        MergeOutcome::Conflict { files, .. } => assert_eq!(files, ["README.md"]),
        other => panic!("expected a conflict, got {other:?}"),
    }
    // Undone: the branch and the files are as they were.
    assert_eq!(sh(&a, &["rev-parse", "HEAD"]), tip);
    assert_eq!(
        fs::read_to_string(a.join("README.md")).unwrap(),
        "a's version\n"
    );
    assert_eq!(sh(&a, &["status", "--porcelain"]), "");
}

#[test]
fn a_commit_puts_a_worktrees_work_on_its_branch_so_a_merge_carries_it() {
    let (dir, git) = project_with_history();
    let main = dir.path();
    let wt = git
        .create_worktree(AgentId::from_u128(3), "x100/c", "main")
        .unwrap();
    let before = sh(&wt, &["rev-parse", "HEAD"]).trim().to_owned();

    // Nothing changed: no commit, and the branch stays where it was.
    assert_eq!(git.commit(&wt, "nothing").unwrap(), None);
    assert_eq!(sh(&wt, &["rev-parse", "HEAD"]).trim(), before);

    write(&wt.join(".gitignore"), "*.log\n");
    write(&wt.join("new/file.txt"), "untracked\n");
    write(&wt.join("README.md"), "edited\n");
    fs::remove_file(wt.join("src/old.rs")).unwrap();
    chmod(&wt.join("src/main.rs"), 0o755);
    write(&wt.join("debug.log"), "ignored\n");
    write(&wt.join("staged.txt"), "staged\n");
    sh(&wt, &["add", "staged.txt"]);

    let commit = git
        .commit(&wt, "x100: work of c")
        .unwrap()
        .expect("a commit");
    assert_eq!(sh(&wt, &["rev-parse", "x100/c"]).trim(), commit.0);
    assert_eq!(sh(&wt, &["rev-parse", "HEAD^"]).trim(), before);
    assert_eq!(
        sh(&wt, &["log", "-1", "--format=%s"]).trim(),
        "x100: work of c"
    );
    // Everything but the ignored file is committed.
    assert_eq!(sh(&wt, &["status", "--porcelain", "-uall"]), "");
    assert!(!sh(&wt, &["ls-files"]).contains("debug.log"));

    // The merge of the branch carries all of it to main.
    let MergeOutcome::Merged { .. } = git.merge(main, "x100/c").unwrap() else {
        panic!("expected a fast-forward");
    };
    let tree = files_of(main);
    assert_eq!(tree["new/file.txt"].1, b"untracked\n");
    assert_eq!(tree["README.md"].1, b"edited\n");
    assert_eq!(tree["staged.txt"].1, b"staged\n");
    assert_eq!(tree["src/main.rs"].0, 0o755);
    assert!(!tree.contains_key("src/old.rs") && !tree.contains_key("debug.log"));
}
