//! `decision_model.*`: connecting the decision model that classifies, and disconnecting it.
//!
//! Connecting puts the key in the vault as a credential scoped to the provider's host and to no
//! project, so the proxy gives it to no driver and only the daemon's own call can use it; then
//! the daemon's setting names it. A model connected before is replaced, and its key removed.
//! Each step is undone if the log refuses the next, as `credentials.*` does.

use std::collections::BTreeSet;

use x100_core::credential::{Credential, CredentialScope, Projects};
use x100_core::decision::{DecisionModel, DecisionProvider};
use x100_core::event::DaemonEvent;
use x100_core::id::CredentialRef;
use x100_core::party::Actor;
use x100_core::settings::DaemonSetting;
use x100_proto::{
    ConnectDecisionModel, DecisionModelConnected, DisconnectDecisionModel, Done, RpcError,
};
use x100_store::DaemonLog;
use x100_vault::{SecretValue, Vault};

use super::credentials::vault_error;
use super::{blocking, store_error};
use crate::hub::Hub;

fn name(provider: DecisionProvider) -> &'static str {
    match provider {
        DecisionProvider::Gemini => "Gemini Decision 1 key",
        DecisionProvider::OpenAi => "OpenAI Decisions key",
        DecisionProvider::Jev => "Jev key",
    }
}

pub async fn connect(
    hub: &Hub,
    p: ConnectDecisionModel,
) -> Result<DecisionModelConnected, RpcError> {
    let credential = Credential {
        reference: x100_orch::new_id(&*hub.clock, CredentialRef::new),
        name: name(p.provider).to_owned(),
        scope: CredentialScope {
            hosts: BTreeSet::from([p.provider.host().to_owned()]),
            commands: BTreeSet::new(),
            projects: Projects::Only(BTreeSet::new()),
        },
    };
    let reference = credential.reference;
    let value = SecretValue::from(p.key.expose());
    let connected = DecisionModel {
        provider: p.provider,
        credential: reference,
    };
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        let before = log.with_state(|d| d.settings().decision_model);
        vault.add(credential.clone(), value).map_err(vault_error)?;
        let undo = |e: x100_store::Error| {
            let _ = vault.remove(reference);
            store_error(&e)
        };
        log.append(Actor::Builder, DaemonEvent::CredentialAdded(credential))
            .map_err(undo)?;
        let set = DaemonEvent::SettingChanged(DaemonSetting::DecisionModel(Some(connected)));
        if let Err(e) = log.append(Actor::Builder, set) {
            forget(&vault, &log, reference);
            return Err(store_error(&e));
        }
        if let Some(old) = before {
            forget(&vault, &log, old.credential);
        }
        Ok(())
    })
    .await?;
    Ok(DecisionModelConnected {
        credential: reference,
    })
}

/// With none connected this changes nothing.
pub async fn disconnect(hub: &Hub, _: DisconnectDecisionModel) -> Result<Done, RpcError> {
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        let Some(old) = log.with_state(|d| d.settings().decision_model) else {
            return Ok(());
        };
        let event = DaemonEvent::SettingChanged(DaemonSetting::DecisionModel(None));
        log.append(Actor::Builder, event)
            .map_err(|e| store_error(&e))?;
        forget(&vault, &log, old.credential);
        Ok(())
    })
    .await?;
    Ok(Done {})
}

/// Removes a key that is no longer the decision model's. Best effort: a value the log still
/// names is reconciled away at the next start, and one it no longer names is removed then too.
fn forget(vault: &Vault, log: &DaemonLog, credential: CredentialRef) {
    if let Err(e) = vault.remove(credential) {
        eprintln!("x100d: the old decision model key {credential} could not be removed: {e}");
        return;
    }
    if let Err(e) = log.append(
        Actor::Builder,
        DaemonEvent::CredentialRemoved { credential },
    ) {
        eprintln!("x100d: the removal of {credential} is not in the log: {e}");
    }
}

/// When the builder removes the decision model's key from Credentials, the model is
/// disconnected with it, so the setting never names a key that is gone.
pub(super) fn removed(log: &DaemonLog, credential: CredentialRef) -> Result<(), RpcError> {
    let named = log.with_state(|d| {
        d.settings()
            .decision_model
            .is_some_and(|m| m.credential == credential)
    });
    if !named {
        return Ok(());
    }
    let event = DaemonEvent::SettingChanged(DaemonSetting::DecisionModel(None));
    log.append(Actor::Builder, event)
        .map(drop)
        .map_err(|e| store_error(&e))
}
