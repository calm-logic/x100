//! The daemon's log for the scenario: one machine, two harnesses, credentials and cool-offs.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::credential::{Credential, CredentialScope, Projects};
use crate::effort::Effort;
use crate::event::{DaemonEvent, Envelope, Seq};
use crate::harness::{
    CoolOff, Harness, HarnessKind, HarnessModel, Machine, ModelName, ModelOffer, ProviderFailure,
    Support,
};
use crate::id::{CredentialRef, HarnessId, JobId, MachineId, ProjectId};
use crate::job::{DaemonJob, Job, JobEvent, Schedule};
use crate::money::{LongContext, Money, Price, Rates};
use crate::party::Actor;
use crate::settings::DaemonSetting;
use crate::time::Timestamp;

pub const MACHINE: MachineId = MachineId::from_u128(1);
pub const CLAUDE: HarnessId = HarnessId::from_u128(2);
pub const CODEX: HarnessId = HarnessId::from_u128(3);

fn on(harness: HarnessId, model: &str) -> HarnessModel {
    HarnessModel {
        harness,
        model: ModelName(model.into()),
    }
}

pub fn opus() -> HarnessModel {
    on(CLAUDE, "claude-opus")
}

pub fn sonnet() -> HarnessModel {
    on(CLAUDE, "claude-sonnet")
}

pub fn gpt() -> HarnessModel {
    on(CODEX, "gpt-codex")
}

fn rates(input: u64, output: u64) -> Rates {
    Rates {
        input: Money::from_micros(input),
        output: Money::from_micros(output),
        cache_read: Money::from_micros(input / 10),
        cache_write_short: Money::from_micros(input * 5 / 4),
        cache_write_long: Money::from_micros(input * 2),
    }
}

fn offer(model: &str, levels: &[Effort], input: u64, output: u64, long: bool) -> ModelOffer {
    let price = Price {
        rates: rates(input, output),
        long_context: long.then(|| LongContext {
            above: 200_000,
            rates: rates(input * 2, output * 3 / 2),
        }),
    };
    ModelOffer {
        model: ModelName(model.into()),
        context_window: if long { 1_000_000 } else { 200_000 },
        cache_lifetime: Duration::from_secs(300),
        levels: levels.iter().map(|e| (*e, price)).collect(),
    }
}

fn claude() -> Harness {
    Harness {
        id: CLAUDE,
        kind: HarnessKind::ClaudeCode,
        support: Support {
            further_turns: true,
            compaction: true,
            context_fill: true,
            permission_hook: true,
        },
        models: vec![
            offer(
                "claude-opus",
                &[Effort::High, Effort::Max],
                5_000_000,
                25_000_000,
                true,
            ),
            offer(
                "claude-sonnet",
                &[Effort::Low, Effort::Medium],
                3_000_000,
                15_000_000,
                false,
            ),
        ],
        login: None,
    }
}

fn codex(cache_lifetime: Duration) -> Harness {
    let mut gpt = offer("gpt-codex", &Effort::ALL, 1_250_000, 10_000_000, false);
    gpt.cache_lifetime = cache_lifetime;
    Harness {
        id: CODEX,
        kind: HarnessKind::Codex,
        support: Support {
            further_turns: true,
            compaction: false,
            context_fill: true,
            permission_hook: false,
        },
        models: vec![gpt],
        login: None,
    }
}

/// The scenario project's id on the daemon's list.
pub const FERNLEAF: ProjectId = ProjectId::from_u128(10);

/// The daemon's log: the machine and harnesses found, the project added, two credentials, a
/// changed setting, and two cool-offs, one ended by its job and one still in force.
pub fn daemon_log(start: Timestamp) -> Vec<Envelope<DaemonEvent>> {
    use DaemonEvent as D;
    let stripe = CredentialRef::from_u128(20);
    let cloudflare = CredentialRef::from_u128(21);
    let scope = |hosts: &[&str], commands: &[&str]| CredentialScope {
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        commands: commands.iter().map(|c| c.to_string()).collect(),
        projects: Projects::Only(BTreeSet::from([FERNLEAF])),
    };
    let cool_off = |of: HarnessModel, until: Timestamp| CoolOff {
        of,
        why: ProviderFailure::RateLimited { retry_after: None },
        until,
    };
    let minute = Duration::from_secs(60);
    let first_end = start + 5 * minute;
    let events = vec![
        D::MachineAdded(Machine {
            id: MACHINE,
            name: "laptop".into(),
            harnesses: vec![CLAUDE, CODEX],
            cap: 8,
            passed_over: Vec::new(),
        }),
        D::HarnessAdded(claude()),
        D::HarnessAdded(codex(Duration::from_secs(300))),
        D::ProjectAdded { project: FERNLEAF },
        D::CredentialAdded(Credential {
            reference: stripe,
            name: "Stripe test key".into(),
            scope: scope(&["api.stripe.com"], &[]),
        }),
        D::CredentialAdded(Credential {
            reference: cloudflare,
            name: "Cloudflare token".into(),
            scope: scope(&[], &[]),
        }),
        D::CredentialScopeSet {
            credential: cloudflare,
            scope: scope(&["api.cloudflare.com"], &["wrangler"]),
        },
        D::CredentialReplaced { credential: stripe },
        D::SettingChanged(DaemonSetting::StallTimeout(10 * minute)),
        D::CoolOffStarted {
            cool_off: cool_off(gpt(), first_end),
            ends: JobId::from_u128(30),
        },
        D::JobScheduled(Job {
            id: JobId::from_u128(30),
            when: Schedule::Once { at: first_end },
            does: DaemonJob::EndCoolOff(gpt()),
        }),
        D::Job {
            job: JobId::from_u128(30),
            event: JobEvent::Fired { at: first_end },
        },
        D::CoolOffEnded(gpt()),
        D::HarnessUpdated(codex(Duration::from_secs(3_600))),
        D::MachineCapSet {
            machine: MACHINE,
            cap: 6,
        },
        D::CoolOffStarted {
            cool_off: cool_off(sonnet(), start + 60 * minute),
            ends: JobId::from_u128(31),
        },
        D::JobScheduled(Job {
            id: JobId::from_u128(31),
            when: Schedule::Once {
                at: start + 60 * minute,
            },
            does: DaemonJob::EndCoolOff(sonnet()),
        }),
        D::CredentialRemoved { credential: stripe },
    ];
    events
        .into_iter()
        .enumerate()
        .map(|(i, event)| Envelope {
            seq: Seq(i as u64 + 1),
            at: start + Duration::from_secs(i as u64 * 30),
            by: if matches!(
                event,
                D::CredentialAdded(_)
                    | D::CredentialScopeSet { .. }
                    | D::CredentialReplaced { .. }
                    | D::CredentialRemoved { .. }
                    | D::SettingChanged(_)
                    | D::ProjectAdded { .. }
            ) {
                Actor::Builder
            } else {
                Actor::Daemon
            },
            event,
        })
        .collect()
}
