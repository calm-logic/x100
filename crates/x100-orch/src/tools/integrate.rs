//! The CEO's integration tools: `list_integrations`, `link_project` and `deploy`. Like every
//! tool, each is decided from the state alone; what Homebox said is asked before the call is
//! decided and handed in on the context. A deploy takes minutes, so its plan only names it, and
//! the daemon does it in the background ([`crate::integrate`]).

use serde_json::{Value, json};
use x100_core::action::{ActionState, Request};
use x100_core::event::ProjectEvent;
use x100_core::integration::{GitHubWay, HomeboxLink, HomeboxWay};
use x100_core::party::Actor;
use x100_core::project::SettingChange;
use x100_core::tool::ToolCall;
use x100_integrations::{Integrations, NO_HOMEBOX};

use super::{At, DeployJob, Plan, Refusal};

pub(super) fn list(at: &At) -> Result<Plan, Refusal> {
    let s = at.ctx.settings;
    let homebox = match &s.homebox {
        None => json!({ "connected": false, "note": NO_HOMEBOX }),
        Some(c) => {
            let mut h = json!({
                "connected": true,
                "way": match c.way {
                    HomeboxWay::Local => "local",
                    HomeboxWay::Address => "address",
                },
                "address": c.address,
                "primary_domain": c.primary_domain,
            });
            match at.ctx.homebox {
                Some(Ok(projects)) => {
                    h["projects"] = projects
                        .iter()
                        .map(|p| {
                            json!({
                                "name": p.name,
                                "repository": p.repo_full_name,
                                "managed": p.managed,
                            })
                        })
                        .collect();
                }
                Some(Err(why)) => h["projects_unavailable"] = json!(why),
                None => {}
            }
            h
        }
    };
    let github = match &s.github {
        None => json!({ "connected": false, "note": x100_integrations::NO_GITHUB }),
        Some(g) => json!({
            "connected": true,
            "account": g.account,
            "way": match g.way {
                GitHubWay::LocalGh => "local gh",
                GitHubWay::DeviceFlow { .. } => "OAuth app",
            },
        }),
    };
    let project = json!({
        "homebox": at.project.homebox.as_ref().map(link_json),
        "repository": at.project.repository,
    });
    Ok(Plan {
        reply: Some(
            json!({ "homebox": homebox, "github": github, "project": project }).to_string(),
        ),
        ..Plan::default()
    })
}

pub(super) fn link(at: &At, wanted: &str, environment: Option<&str>) -> Result<Plan, Refusal> {
    let conn = at.ctx.settings.homebox.as_ref().ok_or(NO_HOMEBOX)?;
    let projects = match at.ctx.homebox {
        Some(Ok(projects)) => projects,
        Some(Err(why)) => return Err(format!("Homebox could not be asked: {why}")),
        None => return Err("Homebox's projects were not read".into()),
    };
    let link = Integrations::link_to(conn, projects, wanted, environment)?;
    Ok(Plan {
        reply: Some(json!({ "linked": link_json(&link) }).to_string()),
        events: vec![(
            Actor::Agent(at.id()),
            ProjectEvent::SettingChanged(SettingChange::Homebox(Some(link))),
        )],
        ..Plan::default()
    })
}

/// Refused before the gate when it could not be done: so the builder is never asked to approve a
/// deploy that would fail at once for want of a connection.
pub(super) fn deploy(at: &At, environment: Option<String>) -> Result<Plan, Refusal> {
    Integrations::can_deploy(at.ctx.settings)?;
    if at.project.code_dir.is_none() {
        return Err("the project has no code yet: set it up first".into());
    }
    if at.ctx.unmerged {
        return Err(UNMERGED.into());
    }
    if under_way(at) {
        return Err(
            "a deploy of this project is already under way: its result comes to your \
                    inbox"
                .into(),
        );
    }
    Ok(Plan {
        deploy: Some(DeployJob { environment }),
        ..Plan::default()
    })
}

/// Why a deploy waits for the caller's report: it ships main, which the caller's work reaches only
/// when its task is reported.
const UNMERGED: &str = "your work is not on the project's main branch yet, and a deploy ships \
     main as it is. `report` your task first, which puts your work on main, then call `deploy` \
     again in the same turn.";

/// Whether a deploy was let through and has not finished: two at once would push and deploy
/// twice.
fn under_way(at: &At) -> bool {
    at.state.actions().any(|a| {
        matches!(a.state, ActionState::Allowed | ActionState::Approved)
            && matches!(
                &a.action.request,
                Request::Tool {
                    call: ToolCall::Deploy { .. },
                    ..
                }
            )
    })
}

fn link_json(link: &HomeboxLink) -> Value {
    json!({
        "homebox_project": link.project_name,
        "repository": link.repo_full_name,
        "environment": link.environment,
        "homebox": link.instance,
    })
}
