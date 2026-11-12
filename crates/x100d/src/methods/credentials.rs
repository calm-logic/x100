//! `credentials.*`: a value goes into the vault and never comes out, and the daemon's log records
//! the credential's name and scope. Each change is made in the vault first and undone there if the
//! log refuses it, so the log never names a credential the vault lacks; see `crate::vault`.

use x100_core::credential::Credential;
use x100_core::event::DaemonEvent;
use x100_core::id::CredentialRef;
use x100_core::party::Actor;
use x100_proto::{
    AddCredential, CredentialAdded, Done, ErrorCode, RemoveCredential, ReplaceCredential, RpcError,
    ScopeCredential,
};
use x100_vault::{SecretValue, VaultError};

use super::{blocking, store_error};
use crate::hub::Hub;

fn vault_error(e: VaultError) -> RpcError {
    match e {
        VaultError::Unknown(r) => RpcError::new(ErrorCode::NotFound, format!("no credential {r}")),
        VaultError::TooShort(_) | VaultError::Exists(_) => {
            RpcError::new(ErrorCode::Rejected, e.to_string())
        }
        other => RpcError::new(ErrorCode::Internal, other.to_string()),
    }
}

pub async fn add(hub: &Hub, p: AddCredential) -> Result<CredentialAdded, RpcError> {
    if p.name.trim().is_empty() {
        return Err(RpcError::new(
            ErrorCode::Rejected,
            "a credential needs a name",
        ));
    }
    let credential = Credential {
        reference: x100_orch::new_id(&*hub.clock, CredentialRef::new),
        name: p.name,
        scope: p.scope,
    };
    let reference = credential.reference;
    let value = SecretValue::from(p.value.expose());
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        vault.add(credential.clone(), value).map_err(vault_error)?;
        log.append(Actor::Builder, DaemonEvent::CredentialAdded(credential))
            .map_err(|e| {
                let _ = vault.remove(reference);
                store_error(&e)
            })
    })
    .await?;
    Ok(CredentialAdded {
        credential: reference,
    })
}

/// A new value cannot be undone if the log refuses it, since the old one is gone; the log records
/// no value, so the two still agree on what exists.
pub async fn replace(hub: &Hub, p: ReplaceCredential) -> Result<Done, RpcError> {
    let value = SecretValue::from(p.value.expose());
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        vault.replace(p.credential, value).map_err(vault_error)?;
        let event = DaemonEvent::CredentialReplaced {
            credential: p.credential,
        };
        log.append(Actor::Builder, event)
            .map_err(|e| store_error(&e))
    })
    .await?;
    Ok(Done {})
}

pub async fn set_scope(hub: &Hub, p: ScopeCredential) -> Result<Done, RpcError> {
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        let old = vault
            .get(p.credential)
            .ok_or_else(|| vault_error(VaultError::Unknown(p.credential)))?;
        vault
            .set_scope(p.credential, p.scope.clone())
            .map_err(vault_error)?;
        let event = DaemonEvent::CredentialScopeSet {
            credential: p.credential,
            scope: p.scope,
        };
        log.append(Actor::Builder, event).map_err(|e| {
            let _ = vault.set_scope(p.credential, old.scope);
            store_error(&e)
        })
    })
    .await?;
    Ok(Done {})
}

/// A removed value cannot be put back if the log refuses the removal; the daemon's next start
/// records it removed.
pub async fn remove(hub: &Hub, p: RemoveCredential) -> Result<Done, RpcError> {
    let (vault, log) = (hub.vault.clone(), hub.log.clone());
    blocking(move || {
        vault.remove(p.credential).map_err(vault_error)?;
        let event = DaemonEvent::CredentialRemoved {
            credential: p.credential,
        };
        log.append(Actor::Builder, event)
            .map_err(|e| store_error(&e))
    })
    .await?;
    Ok(Done {})
}
