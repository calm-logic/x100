//! The vault in the daemon: the store, the authority the proxy ends TLS with, and the proxy, made
//! once at start; the check that the daemon's log and the vault agree; and the proxy's reports,
//! each recorded in the requester's project.

use std::path::PathBuf;

use tokio::sync::mpsc;
use x100_core::event::DaemonEvent;
use x100_core::party::Actor;
use x100_orch::{EgressEnd, Orchestrator};
use x100_store::DaemonLog;
use x100_vault::{
    EgressOutcome, EgressReport, Keychain, NoKeychain, OsKeychain, Proxy, ProxyConfig,
    UpstreamTrust, Vault,
};

use crate::config::Config;
use crate::error::Error;

pub struct Opened {
    pub vault: Vault,
    pub proxy: Proxy,
    /// The authority's public certificate, which sandboxes trust.
    pub certificate: PathBuf,
    pub reports: mpsc::UnboundedReceiver<EgressReport>,
}

/// Opens the vault in `<data dir>/vault`, its key in the OS keychain where there is one. Blocks:
/// it reads files and may talk to D-Bus.
pub fn open(config: &Config) -> Result<Opened, Error> {
    let keychain: Box<dyn Keychain> = if config.no_keychain {
        Box::new(NoKeychain)
    } else {
        Box::new(OsKeychain::connect())
    };
    let vault = Vault::open(&config.data_dir.join("vault"), &*keychain)?;
    let authority = vault.authority()?;
    let certificate = authority.certificate_path().to_path_buf();
    let proxy_config = ProxyConfig {
        upstream: UpstreamTrust::System,
        resolve: config.proxy_resolve.clone(),
        closed: !config.proxy_resolve.is_empty(),
    };
    let (proxy, reports) = Proxy::new(vault.clone(), authority, proxy_config)?;
    Ok(Opened {
        vault,
        proxy,
        certificate,
        reports,
    })
}

/// The log is the record of what credentials exist and where they may go; the vault holds the
/// values. A change is written to the vault first and then to the log, and undone in the vault if
/// the log refuses it, so they disagree only after a crash between the two: a value the log never
/// recorded is removed, a credential whose value is gone is recorded removed, and a scope is set
/// to the log's. Blocks.
pub fn reconcile(vault: &Vault, log: &DaemonLog) -> Result<(), Error> {
    let logged: Vec<_> =
        log.with_state(|d| d.credentials().map(|r| r.credential.clone()).collect());
    for held in vault.list() {
        match logged.iter().find(|c| c.reference == held.reference) {
            None => {
                eprintln!(
                    "x100d: credential {} is in the vault but not in the log; removed",
                    held.reference
                );
                vault.remove(held.reference)?;
            }
            Some(c) if c.scope != held.scope => {
                eprintln!(
                    "x100d: credential {}'s scope differs from the log's; set to the log's",
                    held.reference
                );
                vault.set_scope(held.reference, c.scope.clone())?;
            }
            Some(_) => {}
        }
    }
    for c in logged {
        if vault.get(c.reference).is_none() {
            eprintln!(
                "x100d: credential {} is in the log but its value is gone; recorded removed",
                c.reference
            );
            log.append(
                Actor::Daemon,
                DaemonEvent::CredentialRemoved {
                    credential: c.reference,
                },
            )?;
        }
    }
    Ok(())
}

/// Records each request the proxy reports in the requester's project: how the gate's action for
/// it ended, or the whole request when the gate was not asked, with the credential uses it made.
/// Runs until the proxy is gone.
pub fn drain(
    mut reports: mpsc::UnboundedReceiver<EgressReport>,
    orch: Orchestrator,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(report) = reports.recv().await {
            let end = match &report.outcome {
                EgressOutcome::Answered(_) | EgressOutcome::Tunnelled => EgressEnd::Went,
                EgressOutcome::HostRefused => EgressEnd::RefusedByProxy,
                EgressOutcome::NotAdmitted => EgressEnd::NotAdmitted,
                EgressOutcome::Failed(why) => EgressEnd::Failed(why.clone()),
            };
            let requester = report.requester;
            orch.egress(
                requester.project,
                requester.agent,
                report.request(),
                report.action,
                report.credential_uses(),
                end,
            )
            .await;
        }
    })
}
