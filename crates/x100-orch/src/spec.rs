//! What a driver is started with besides its model: where it works, how it reaches the daemon's
//! tools, the builder's login it runs on, and what it may do without asking.

use std::path::PathBuf;
use std::sync::Arc;

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

pub(crate) fn build(
    shared: &Shared,
    ctx: &Arc<ProjectCtx>,
    agent: AgentId,
    driver: DriverId,
    choice: &Choice,
    start_pack: String,
) -> DriverSpec {
    let dir = ctx.store.git.worktree_path(agent);
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
