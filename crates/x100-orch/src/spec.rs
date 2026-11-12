//! What a driver is started with besides its model: where it works, how it reaches the daemon's
//! tools, the builder's login it runs on, and what it may do without asking.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_core::credential::Projects;
use x100_core::driver::Purpose;
use x100_core::id::{AgentId, DriverId};
use x100_drivers::{DriverSpec, Harness, Permissions, SpawnHook, StallPolicy};
use x100_router::Choice;
use x100_vault::Requester;

use crate::Shared;
use crate::ctx::ProjectCtx;
use crate::sandbox::Placed;

/// What every driver may do without asking, set here and nowhere else.
///
/// Every driver runs in its agent's sandbox, which shows it its worktree, a home of its own and
/// the programs it runs, and nothing else of the builder's, and whose only way out is the vault's
/// proxy. So the harnesses' own permission prompts and their own sandboxes are off: a command
/// runs inside x100's sandbox without asking, and the daemon's own tools are still gated by the
/// desk. A daemon whose builder turned on `run_without_sandbox` runs drivers like this with no
/// sandbox, which that setting says is unsafe. Nothing prompts on the terminal: nobody can type
/// into it.
pub(crate) const PERMISSIONS: Permissions = Permissions::AllowAll;

/// What a model is told about where it works, before it plans work it could not carry out. A
/// driver started before the project has code works in a scratch directory, and is told so.
pub(crate) fn workspace(scratch: bool, contained: bool) -> String {
    let place = if scratch {
        "Your working directory is a scratch directory: the project is not set up yet and has no \
         code. Nothing you write there is kept, so do not start work in it."
    } else {
        "Your working directory is your own copy of the project, on a git branch of its own. \
         Change files only there; they are committed when you report."
    };
    let rules = if contained {
        "You run in a sandbox of your own: you may run commands, install and build what the work \
         needs, and change anything in your working directory and your home. Nothing else of \
         the machine is there. Web requests go out through x100's proxy. To leave a server or \
         other long-running command running after your turn, start it with `start_process`, \
         since commands you start yourself end with you."
    } else {
        "You run without a sandbox, on the builder's own machine and account: run only what the \
         work needs, and change nothing outside your working directory."
    };
    format!("{place} {rules}")
}

/// `dir` is the agent's worktree, or the project's scratch directory before it has code.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    shared: &Shared,
    ctx: &Arc<ProjectCtx>,
    harness: &dyn Harness,
    driver: DriverId,
    choice: &Choice,
    dir: PathBuf,
    start_pack: String,
    placed: &Placed,
) -> DriverSpec {
    // The path is passed on to the harness, which reaches the login there; x100 never opens it.
    let login = shared.daemon.with_state(|d| {
        d.harness(choice.harness)
            .and_then(|h| h.login.as_ref())
            .map(|l| PathBuf::from(&l.0))
    });
    let mut env = placed.env.clone();
    env.extend(api_key(shared, ctx, harness));
    DriverSpec {
        driver,
        model: choice.model.clone(),
        effort: choice.effort,
        dir,
        state: placed.state.clone(),
        wrap: placed.wrap.clone(),
        expose: placed.expose.clone(),
        tmp: placed.tmp.clone(),
        start_pack,
        tools: Some(shared.desk.issue(driver, Arc::clone(ctx))),
        env,
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

/// Puts `driver` in its agent's sandbox, and records the processes of a sandbox that had to be
/// replaced, such as the scratch directory's once the project has code.
pub(crate) async fn place(
    shared: &Shared,
    ctx: &Arc<ProjectCtx>,
    agent: AgentId,
    driver: DriverId,
    harness: &dyn Harness,
    dir: &Path,
) -> Result<Placed, String> {
    let requester = Requester {
        project: ctx.id,
        agent,
        driver,
        purpose: Purpose::Work,
    };
    let placed = shared
        .sandboxes
        .place(
            &(shared.settings)(),
            requester,
            dir,
            shared.desk.tools_addr(),
            logins(shared),
            &harness.login_hosts(),
            &provider_hosts(harness),
        )
        .await?;
    crate::processes_ended(ctx, vec![(agent, placed.ended.clone())]).await;
    Ok(placed)
}

/// The hosts of the model provider a harness's drivers reach: its login's hosts, or the host its
/// API key goes to. The gate is not asked about them, since x100 chose them, not the agent.
fn provider_hosts(harness: &dyn Harness) -> Vec<String> {
    let mut hosts = harness.login_hosts();
    hosts.extend(harness.api_key().map(|k| k.host));
    hosts
}

/// A harness run on an API key gets a reference to the first credential that may go to its
/// provider's host from this project, which the proxy swaps for the value there.
pub(crate) fn api_key(
    shared: &Shared,
    ctx: &ProjectCtx,
    harness: &dyn Harness,
) -> Option<(String, String)> {
    let key = harness.api_key()?;
    shared.daemon.with_state(|d| {
        d.credentials()
            .map(|r| &r.credential)
            .find(|c| {
                let project = match &c.scope.projects {
                    Projects::All => true,
                    Projects::Only(p) => p.contains(&ctx.id),
                };
                project
                    && c.scope
                        .hosts
                        .iter()
                        .any(|h| h.eq_ignore_ascii_case(&key.host))
            })
            .map(|c| (key.env.clone(), c.reference.to_string()))
    })
}

/// The login directories a sandbox shows read-write: every terminal harness's, since an agent's
/// next driver may run on another program than its first.
pub(crate) fn logins(shared: &Shared) -> Vec<PathBuf> {
    shared.daemon.with_state(|d| {
        d.harnesses()
            .filter_map(|h| h.login.as_ref().map(|l| PathBuf::from(&l.0)))
            .collect()
    })
}
