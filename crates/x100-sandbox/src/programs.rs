//! Which host paths hold an installed program, so a sandbox can show it read-only.

use std::path::{Path, PathBuf};

/// The paths that hold `program`, the file a search path found: the whole install for a program
/// under a `bin` directory whose real file is in that install (node, and what npm installs
/// beside it), or else the file found and the directory its link resolves into (a program
/// installed as a link to a versioned directory). Empty if `program` does not exist.
pub fn program_paths(program: &Path) -> Vec<PathBuf> {
    let Ok(real) = program.canonicalize() else {
        return Vec::new();
    };
    let (Some(bin), Some(real_dir)) = (program.parent(), real.parent()) else {
        return vec![real];
    };
    if let Some(root) = bin.parent()
        && bin.file_name().is_some_and(|n| n == "bin")
        // Compared resolved, since the search path itself may run through a link (macOS's /var).
        && let Ok(real_root) = root.canonicalize()
        && (real_dir == real_root.join("bin") || real.starts_with(real_root.join("lib")))
    {
        return vec![root.to_path_buf()];
    }
    let mut paths = vec![program.to_path_buf(), real_dir.to_path_buf()];
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_under_bin_is_shown_whole_and_a_linked_one_by_its_parts() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("node");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("lib/pkg")).unwrap();
        std::fs::write(root.join("lib/pkg/cli.js"), "").unwrap();
        std::os::unix::fs::symlink(root.join("lib/pkg/cli.js"), root.join("bin/tool")).unwrap();
        assert_eq!(program_paths(&root.join("bin/tool")), vec![root.clone()]);

        let versions = base.path().join("share/tool/1.0");
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::write(versions.join("tool"), "").unwrap();
        let links = base.path().join("local/bin");
        std::fs::create_dir_all(&links).unwrap();
        std::os::unix::fs::symlink(versions.join("tool"), links.join("tool")).unwrap();
        assert_eq!(
            program_paths(&links.join("tool")),
            vec![links.join("tool"), versions.canonicalize().unwrap()]
        );
        assert!(program_paths(&links.join("missing")).is_empty());
    }
}
