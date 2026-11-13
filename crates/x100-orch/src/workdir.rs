//! What the desk does outside the log besides git's work: a new project's directory when it is
//! set up, each spec as a file for people to read, and a process started or stopped in an
//! agent's sandbox.

use x100_core::event::ProjectEvent;
use x100_core::spec::SpecKind;
use x100_store::Git;

use x100_vault::Requester;

use crate::sandbox::Sandboxes;
use crate::tools::{NewCodeDir, ProcessPlan};

/// Makes a project's code directory under the projects root, named by its slug, or by the slug and
/// a number when that is taken by a project or anything else, and starts its repository there.
/// The directory is claimed by creating it, so two projects set up at once cannot get the same one.
pub(crate) fn make_code_dir(new: &NewCodeDir) -> Result<ProjectEvent, String> {
    let cannot = |e: &dyn std::fmt::Display| format!("the project's code directory: {e}");
    std::fs::create_dir_all(&new.root).map_err(|e| cannot(&e))?;
    let mut n = 1;
    let dir = loop {
        let name = match n {
            1 => new.slug.clone(),
            _ => format!("{}-{n}", new.slug),
        };
        let dir = new.root.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => break dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(cannot(&e)),
        }
    };
    if let Err(e) = Git::new(dir.clone()).prepare() {
        // Nothing but x100 has used it, since it was made just now.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(cannot(&e));
    }
    Ok(ProjectEvent::CodeDirSet { dir })
}

/// Specs are kept as Markdown in the code directory's `.x100/specs/`, written from the log for
/// people to read. The log is the record, so a file that cannot be written loses nothing.
pub(crate) fn write_spec_file(git: &Git, kind: SpecKind, body: &str) {
    let name = match kind {
        SpecKind::Brd => "BRD.md",
        SpecKind::Prd => "PRD.md",
        SpecKind::Add => "ADD.md",
    };
    let dir = git.project_dir().join(".x100").join("specs");
    if let Err(e) =
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(name), body))
    {
        eprintln!("x100: could not write {name}: {e}");
    }
}

/// Starts or stops a process in the agent's sandbox. Blocks: it runs on a blocking thread with the
/// runtime's handle at hand. A start answers with where the process's output goes.
pub(crate) fn carry_out_process(
    sandboxes: Option<&Sandboxes>,
    requester: Requester,
    process: &ProcessPlan,
) -> Result<Option<String>, String> {
    let sandboxes = sandboxes.ok_or("this daemon runs no sandboxes")?;
    let runtime = tokio::runtime::Handle::current();
    match process {
        ProcessPlan::Start { name, command } => {
            let log = runtime.block_on(sandboxes.start_process(requester, name, command))?;
            Ok(Some(
                serde_json::json!({
                    "process": name,
                    "running": true,
                    "output": log.display().to_string(),
                })
                .to_string(),
            ))
        }
        ProcessPlan::Stop { name } => {
            runtime.block_on(sandboxes.stop_process(requester.agent, name))?;
            Ok(None)
        }
    }
}
