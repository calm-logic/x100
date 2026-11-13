//! The gate's classifier: one request to the decision model the daemon's settings name, for an
//! action no rule covers.
//!
//! The daemon makes the request over HTTPS through the vault's proxy. The key goes in as the
//! credential's reference, which the proxy swaps for the value in the headers alone, so nothing
//! outside the vault handles it. The call is recorded as a `OneOffCalled` event, its input tokens
//! are charged to the acting agent's payer at the provider's price, and it is not made once that
//! payer's budget is spent. No program is started for it.

use std::sync::Arc;
use std::time::Duration;

use x100_core::credential::{CredentialUse, Via};
use x100_core::decision::DecisionModel as Connected;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::harness::{ModelName, OneOffCall};
use x100_core::id::{AgentId, DriverId, OneOffCallId};
use x100_core::ledger::{Charge, ChargeSource, CostBasis, Payer};
use x100_core::money::Usage;
use x100_core::party::Actor;
use x100_gate::decision::{self, DecisionModel};
use x100_gate::{Asked, Question};
use x100_vault::{CallMethod, DaemonCall, Scheme};

use crate::Shared;
use crate::ctx::{Events, ProjectCtx};
use crate::ids::new_id;
use crate::store::blocking;

/// How long one request may take. The action waits meanwhile; past this it is held.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);

/// A provider that says it is busy is asked again after each of these waits, then given up on.
const AGAIN_AFTER: [Duration; 2] = [Duration::from_millis(500), Duration::from_millis(1500)];

/// What came of asking about `question`, the action of `driver`, and the events that record
/// the call, its charge and the key's use.
pub(crate) async fn classify(
    shared: &Arc<Shared>,
    ctx: &Arc<ProjectCtx>,
    question: &Question,
    driver: DriverId,
) -> (Asked, Events) {
    let Some(connected) = (shared.settings)().decision_model else {
        return (Asked::NotConnected, Vec::new());
    };
    let agent = question.agent;
    let payer = match read(ctx, move |s| payer(s, agent)).await {
        Ok(payer) => payer,
        Err(why) => return (Asked::NoAnswer { call: None, why }, Vec::new()),
    };
    let Some(proxy) = shared.sandboxes.proxy() else {
        let why = "this daemon has no way out to the decision model".to_owned();
        return (Asked::NoAnswer { call: None, why }, Vec::new());
    };
    let model = decision::model(connected.provider);
    let call = new_id(&*ctx.clock, OneOffCallId::new);
    let (asked, input_tokens, uses) = ask(proxy, model, connected, question, call).await;
    let mut events: Events = vec![(
        Actor::Daemon,
        ProjectEvent::OneOffCalled(OneOffCall {
            id: call,
            purpose: question.purpose(),
            provider: connected.provider,
            model: ModelName(model.model().to_owned()),
        }),
    )];
    if let Some(tokens) = input_tokens {
        let usage = Usage {
            input: tokens,
            ..Usage::default()
        };
        events.push((
            Actor::Daemon,
            ProjectEvent::Charged(Charge {
                payer,
                source: ChargeSource::OneOff { call },
                usage,
                cost: model.price().cost(&usage),
                basis: CostBasis::Listed,
            }),
        ));
    }
    events.extend(uses.into_iter().map(|outcome| {
        let used = CredentialUse {
            credential: connected.credential,
            agent,
            driver,
            via: Via::Proxy {
                host: connected.provider.host().to_owned(),
            },
            outcome,
        };
        (Actor::Daemon, ProjectEvent::CredentialUsed(used))
    }));
    (asked, events)
}

/// The acting agent's payer, if its budget is not spent.
fn payer(s: &ProjectState, agent: AgentId) -> Result<Payer, String> {
    let project = s.project().ok_or("the project is not in the log")?;
    let payer = project
        .team
        .payer(agent)
        .ok_or("the agent is not on the team")?;
    let budget = s.budget().ok_or("the project has no budget")?;
    let line = match payer {
        Payer::Project => Some(&budget.project),
        Payer::Lead(lead) => budget.leads.get(&lead),
        Payer::Lab => Some(&budget.lab),
    };
    if line.is_none_or(|l| l.stopped) {
        return Err(match payer {
            Payer::Lead(_) => "the lead's budget is spent, so it was not asked".into(),
            Payer::Lab => "the lab's budget is spent, so it was not asked".into(),
            Payer::Project => "the project's budget is spent, so it was not asked".into(),
        });
    }
    Ok(payer)
}

/// Sends the request, again after a short wait while the provider says it is busy. Returns what
/// came of it, the input tokens the provider counted over every attempt, and what became of the
/// key on each.
async fn ask(
    proxy: &x100_vault::Proxy,
    model: &'static dyn DecisionModel,
    connected: Connected,
    question: &Question,
    call: OneOffCallId,
) -> (Asked, Option<u64>, Vec<x100_core::credential::UseOutcome>) {
    let (header, value) = model.key_header(&connected.credential.to_string());
    let request = DaemonCall {
        method: CallMethod::Post,
        scheme: Scheme::Https,
        host: connected.provider.host().to_owned(),
        port: None,
        path: model.path().to_owned(),
        credential: Some(connected.credential),
        headers: vec![
            (header.to_owned(), value),
            ("content-type".into(), "application/json".into()),
        ],
        body: model.body(question).to_string().into_bytes(),
        timeout: ANSWER_WITHIN,
    };
    let mut tokens: Option<u64> = None;
    let mut uses = Vec::new();
    let mut waits = AGAIN_AFTER.iter();
    loop {
        let called = proxy.call(request.clone()).await;
        uses.extend(called.used);
        let (status, body) = match called.answer {
            Ok(answer) => answer,
            Err(why) => {
                return (
                    Asked::NoAnswer {
                        call: Some(call),
                        why,
                    },
                    tokens,
                    uses,
                );
            }
        };
        let reply = model.read(status, &body);
        if let Some(n) = reply.input_tokens {
            tokens = Some(tokens.unwrap_or(0).saturating_add(n));
        }
        match reply.answer {
            Ok(labels) => return (Asked::Answered { call, labels }, tokens, uses),
            Err(no) => match waits.next().filter(|_| no.again) {
                Some(wait) => tokio::time::sleep(*wait).await,
                None => {
                    let asked = Asked::NoAnswer {
                        call: Some(call),
                        why: no.why,
                    };
                    return (asked, tokens, uses);
                }
            },
        }
    }
}

async fn read<T: Send + 'static>(
    ctx: &Arc<ProjectCtx>,
    f: impl FnOnce(&ProjectState) -> T + Send + 'static,
) -> T {
    let ctx = Arc::clone(ctx);
    blocking(move || ctx.store.log.with_state(f)).await
}
