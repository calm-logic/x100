//! Requests out through the vault's proxy: the gate asked before each goes, and how each ended
//! recorded after, with the credential uses it made.

use std::sync::Arc;

use x100_core::action::{Action, Request};
use x100_core::credential::CredentialUse;
use x100_core::event::ProjectEvent;
use x100_core::id::{ActionId, AgentId, ProjectId};
use x100_core::party::Actor;

use crate::store::blocking;
use crate::{Orchestrator, admit, ctx, gate, new_id, oneoff};

impl Orchestrator {
    /// What the vault's proxy asks before a request goes out: the gate, for this daemon's
    /// projects.
    pub fn admission(&self) -> Arc<dyn x100_vault::Admission> {
        Arc::new(Gatekeeper(self.clone()))
    }

    /// A request a driver sent out through the proxy, as its report tells how it ended. `action`
    /// is the gate's, when the gate was asked: it is ended here. A request the gate was not asked
    /// about, to the driver's own provider or to a host the proxy refuses, is recorded whole.
    /// Either way, with the credential uses it made.
    pub async fn egress(
        &self,
        project: ProjectId,
        agent: AgentId,
        request: Request,
        action: Option<ActionId>,
        uses: Vec<CredentialUse>,
        end: EgressEnd,
    ) {
        if let Some(id) = action {
            self.shared
                .in_flight
                .lock()
                .expect("in-flight lock")
                .remove(&id);
        }
        let Ok(ctx) = self.ctx(project) else {
            return;
        };
        let Request::Egress {
            ref host,
            ref method,
            ..
        } = request
        else {
            return;
        };
        let mut events: ctx::Events = Vec::new();
        let error = match &end {
            EgressEnd::Went => None,
            EgressEnd::Failed(why) => Some(why.clone()),
            EgressEnd::RefusedByProxy => Some(format!("the proxy refuses {host}")),
            EgressEnd::NotAdmitted => None,
        };
        match (action, &end) {
            (Some(_), EgressEnd::NotAdmitted) => {}
            (Some(id), _) => events.push(gate::finished(id, error)),
            (None, EgressEnd::NotAdmitted) => {
                eprintln!("x100: a request to {host} went out before the gate was ready: refused");
            }
            (None, _) => {
                let how = match end {
                    EgressEnd::RefusedByProxy => x100_gate::Settled::RefusedByProxy,
                    _ => x100_gate::Settled::Provider,
                };
                let ruling = x100_gate::settled(how, host, method);
                let action = Action {
                    id: new_id(&*ctx.clock, ActionId::new),
                    agent,
                    request: request.clone(),
                };
                events.extend(gate::proposed(&action, &ruling));
                events.push(gate::finished(action.id, error));
            }
        }
        events.extend(
            uses.into_iter()
                .map(|u| (Actor::Daemon, ProjectEvent::CredentialUsed(u))),
        );
        if events.is_empty() {
            return;
        }
        blocking(move || {
            if let Err(e) = ctx.writer().append(events) {
                eprintln!("x100: an egress of project {project} could not be recorded: {e}");
            }
        })
        .await;
    }
}

/// How a request through the proxy ended, as its report says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressEnd {
    /// It went out and was answered, or its tunnel opened.
    Went,
    Failed(String),
    /// The proxy refuses the host; nothing went out.
    RefusedByProxy,
    /// The gate did not let it through; nothing went out.
    NotAdmitted,
}

/// The gate, as the vault's proxy asks it.
struct Gatekeeper(Orchestrator);

#[async_trait::async_trait]
impl x100_vault::Admission for Gatekeeper {
    async fn admit(
        &self,
        requester: x100_vault::Requester,
        method: &str,
        host: &str,
    ) -> x100_vault::Admit {
        let shared = &self.0.shared;
        if oneoff::is_one_off(shared, requester.driver) {
            return x100_vault::Admit::Refuse {
                action: None,
                why: "x100 refused this: a one-off call reaches its model and nothing else".into(),
            };
        }
        let Ok(ctx) = self.0.ctx(requester.project) else {
            return x100_vault::Admit::Refuse {
                action: None,
                why: "x100 refused this: the project is not open".into(),
            };
        };
        let request = Request::Egress {
            driver: requester.driver,
            host: host.to_owned(),
            method: method.to_owned(),
        };
        match admit::admit(shared, &ctx, requester.agent, request).await {
            admit::Admitted::Go(action) => x100_vault::Admit::Go { action },
            admit::Admitted::Refused { action, why } => x100_vault::Admit::Refuse { action, why },
        }
    }
}
