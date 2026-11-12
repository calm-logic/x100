//! What a driver is started with besides its model: where it works, how it reaches the daemon's
//! tools, the builder's login it runs on, and what it may do without asking.

use std::path::PathBuf;
use std::sync::Arc;

use x100_core::harness::HarnessKind;
use x100_core::id::{AgentId, DriverId};
use x100_drivers::{DriverSpec, Permissions, SpawnHook, StallPolicy};
use x100_router::Choice;

use crate::Shared;
use crate::ctx::ProjectCtx;

/// What every driver may do without asking, set here and nowhere else.
///
/// Until sandboxes exist (M10) a driver runs as the builder, on the builder's machine. With
/// [`Permissions::AllowAll`] a model could run any command the builder could. With this setting a
/// driver changes files in its agent's worktree without asking. Codex runs its commands in its own
/// sandbox, which can write only the worktree and reach no network. Claude Code's own sandbox
/// needs `socat` on Linux and is not relied on, so every command a Claude Code driver would run,
/// and every edit outside its worktree, is put to the daemon, which refuses it until the gate
/// exists (M12). Nothing prompts on the terminal: nobody can type into it.
pub(crate) const PERMISSIONS: Permissions = Permissions::Worktree;

/// What a model is told [`PERMISSIONS`] lets it do with a driver of `harness`, before it plans
/// work it could not carry out. A driver started before the project has code works in a scratch
/// directory, and is told so.
pub(crate) fn workspace(harness: &HarnessKind, scratch: bool) -> String {
    if scratch {
        return "Your working directory is a scratch directory: the project is not set up yet and \
                has no code. Nothing you write there is kept, so do not start work in it."
            .to_owned();
    }
    let base = "Your working directory is your own copy of the project, on a git branch of its \
                own. Change files only there; they are committed when you report.";
    let more = match harness {
        HarnessKind::ClaudeCode | HarnessKind::ClaudeCodeTerminal => {
            "Commands are not available here yet: every shell command is refused, so do not try \
             one, and do not plan to run, install or test anything. Create and edit the files \
             directly with your file tools."
        }
        HarnessKind::Codex | HarnessKind::CodexTerminal => {
            "You may run commands there, in a sandbox that can write only to that directory and \
             /tmp and has no network, so nothing can be installed or downloaded."
        }
        HarnessKind::Custom { .. } => "",
    };
    format!("{base} {more}").trim_end().to_owned()
}

/// What a model is told when the harness asks the daemon about something [`PERMISSIONS`] does not
/// let through, such as a shell command, so it carries on rather than trying again.
pub(crate) const REFUSED: &str = "Commands are not available here yet, and neither are web \
     requests or changes outside your working directory. Do not try again: write the files \
     directly in your working directory with your file tools, and say in your report what you \
     could not run or check.";

/// `dir` is the agent's worktree, or the project's scratch directory before it has code.
pub(crate) fn build(
    shared: &Shared,
    ctx: &Arc<ProjectCtx>,
    agent: AgentId,
    driver: DriverId,
    choice: &Choice,
    dir: PathBuf,
    start_pack: String,
) -> DriverSpec {
    // The path is passed on to the harness, which reaches the login there; x100 never opens it.
    let login = shared.daemon.with_state(|d| {
        d.harness(choice.harness)
            .and_then(|h| h.login.as_ref())
            .map(|l| PathBuf::from(&l.0))
    });
    DriverSpec {
        driver,
        model: choice.model.clone(),
        effort: choice.effort,
        wrap: shared.sandbox.wrap(agent, &dir),
        dir,
        start_pack,
        tools: Some(shared.desk.issue(driver, Arc::clone(ctx))),
        env: Vec::new(),
        stall: StallPolicy::from_settings(&(shared.settings)()),
        permissions: PERMISSIONS,
        login,
        // Each process is written down before any handshake, since a daemon killed during one
        // would otherwise leave a process nobody recorded. A terminal driver of Codex has two. A
        // blocking write, but a short one.
        on_spawn: SpawnHook::new({
            let processes = Arc::clone(&shared.processes);
            move |pid| {
                if let Err(e) = processes.record(driver, pid) {
                    eprintln!("x100: could not write down driver {driver}'s process: {e}");
                }
            }
        }),
    }
}
