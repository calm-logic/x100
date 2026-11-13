//! A deploy, done by the daemon in the background once the gate let the call through or the
//! builder approved it. It takes minutes, so it holds neither the driver's call nor the project's
//! lock. What it makes true on the way (the repository, the link) is recorded at once; the action
//! ends when it is done, and the agent is told through its inbox: as the decision's result for a
//! held call, or as `Finished` for one that was let through.

use std::sync::Arc;

use async_trait::async_trait;
use x100_core::action::{Action, Decision};
use x100_core::event::ProjectEvent;
use x100_core::inbox::InboxItem;
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_core::settings::DaemonSettings;
use x100_integrations::{Deployed, Integrations, ProjectAt, Record};

use crate::ctx::ProjectCtx;
use crate::gate;
use crate::settle::deliver;
use crate::store::blocking;
use crate::tools::DeployJob;

/// What the connected Homebox's projects were found to be, before a call that reads them.
pub(crate) type Prefetched = Option<Result<Vec<crate::tools::HomeboxProject>, String>>;

/// The connected Homebox's projects, for a call that reads them; `None` for any other call, or
/// with no Homebox connected.
pub(crate) async fn prefetch(
    integrations: Option<&Integrations>,
    call: &x100_core::tool::ToolCall,
    settings: &DaemonSettings,
) -> Prefetched {
    use x100_core::tool::ToolCall;
    if !matches!(
        call,
        ToolCall::ListIntegrations {} | ToolCall::LinkProject { .. }
    ) {
        return None;
    }
    let conn = settings.homebox.as_ref()?;
    Some(match integrations {
        Some(i) => i.homebox_projects(conn).await,
        None => Err("this daemon has no integrations".into()),
    })
}

/// What the driver reads at once.
pub(crate) fn started(action: &Action) -> String {
    serde_json::json!({
        "under_way": action.id,
        "message": "The deploy is under way. Its result, with the address or why it failed, \
                    reaches your inbox when it is done; you may end your turn meanwhile.",
    })
    .to_string()
}

/// Starts the deploy. `held` says the builder approved it, so the result is the decision's.
pub(crate) fn start(
    ctx: Arc<ProjectCtx>,
    integrations: Arc<Integrations>,
    settings: DaemonSettings,
    action: Action,
    job: DeployJob,
    held: bool,
) {
    tokio::spawn(async move {
        let result = run(&ctx, &integrations, &settings, &job).await;
        let (error, words) = match result {
            Ok(done) => (None, told(&done)),
            Err(why) => (
                Some(why.clone()),
                format!("The deploy failed: {why}. Tell the builder why."),
            ),
        };
        let item = if held {
            x100_gate::tell_agent(action.id, Decision::Approved, Some(words))
        } else {
            InboxItem::Finished {
                action: action.id,
                result: words,
            }
        };
        let id = action.id;
        let done = blocking(move || {
            let w = ctx.writer();
            let tell = deliver(w.clock(), action.agent, item);
            w.append(vec![gate::finished(id, error), tell]).map(drop)
        })
        .await;
        if let Err(e) = done {
            eprintln!("x100: the end of action {id} is not in the log: {e}");
        }
    });
}

/// The project as it is now, as a deploy needs it.
pub(crate) fn project_at(ctx: &ProjectCtx) -> Option<ProjectAt> {
    ctx.store.log.with_state(|s| {
        let p = s.project()?;
        Some(ProjectAt::new(
            p.code_dir.as_deref()?,
            p.homebox.clone(),
            p.repository.clone(),
        ))
    })
}

async fn run(
    ctx: &Arc<ProjectCtx>,
    integrations: &Integrations,
    settings: &DaemonSettings,
    job: &DeployJob,
) -> Result<Deployed, String> {
    let read = Arc::clone(ctx);
    let project = blocking(move || project_at(&read))
        .await
        .ok_or("the project has no code to deploy")?;
    let record = Recorder {
        ctx: Arc::clone(ctx),
        by: Actor::Daemon,
    };
    integrations
        .deploy(settings, &project, job.environment.as_deref(), &record)
        .await
}

fn told(done: &Deployed) -> String {
    let short: String = done.commit.chars().take(12).collect();
    match &done.address {
        Some(address) => format!(
            "The deploy is live at {address} (commit {short}). Tell the builder the address."
        ),
        None => format!(
            "The deploy is live (commit {short}), but Homebox gives it no public address: it \
             has no public service."
        ),
    }
}

/// Appends each setting a step makes true.
pub(crate) struct Recorder {
    pub ctx: Arc<ProjectCtx>,
    pub by: Actor,
}

#[async_trait]
impl Record for Recorder {
    async fn record(&self, change: SettingChange) -> Result<(), String> {
        let (ctx, by) = (Arc::clone(&self.ctx), self.by);
        blocking(move || {
            ctx.writer()
                .append(vec![(by, ProjectEvent::SettingChanged(change))])
                .map(drop)
                .map_err(|e| format!("the log refused it: {e}"))
        })
        .await
    }
}
