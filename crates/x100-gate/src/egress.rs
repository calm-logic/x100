//! Network requests out of the sandbox, by host and method.
//!
//! Hosts of a known kind (registries, code hosts, messaging, hosting, payments, sign-in) are ruled
//! by what they are. Any other host is ruled by whether the builder has already approved a request
//! to it. A host nobody has approved is not ruled: the classifier is asked, and its answer cannot
//! go below the floor given here.

use x100_core::action::{ActionState, Request, Risk};
use x100_core::fold::ProjectState;

use crate::gate::{Ruling, from_rule};
use crate::rule::{self, Matched, Rule};

/// Egress that x100 settled itself, so the gate records it without judging it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// The harness's own traffic to the model provider x100 started the driver on.
    Provider,
    /// The proxy refused the host before anything went out.
    RefusedByProxy,
    /// A lab driver's read, which goes out without the gate's judging, since a lab driver holds
    /// no credential.
    LabRead,
    /// A lab driver's write, refused before anything goes out.
    LabWrite,
}

impl Settled {
    /// How a lab driver's request is settled: a read goes, a write does not.
    pub fn lab(method: &str) -> Settled {
        if is_read(method) {
            Settled::LabRead
        } else {
            Settled::LabWrite
        }
    }
}

/// The ruling recorded for egress x100 settled: allowed, since the gate holds nothing back from
/// it, with the row that says why.
pub fn settled(how: Settled, host: &str, method: &str) -> Ruling {
    let row = match how {
        Settled::Provider => &rule::EGRESS_PROVIDER,
        Settled::RefusedByProxy => &rule::EGRESS_REFUSED,
        Settled::LabRead => &rule::EGRESS_LAB_READ,
        Settled::LabWrite => &rule::EGRESS_LAB_WRITE,
    };
    let detail = format!("{} {}", method.to_uppercase(), normal(host));
    Ruling::Allow(from_rule(&Matched::because(row, detail)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Matched(Matched),
    /// No row covers it. The classifier decides, but never below `floor`.
    Unknown {
        floor: Risk,
    },
}

enum Method {
    Read,
    Write,
    Only(&'static [&'static str]),
}

struct Row {
    hosts: &'static [&'static str],
    method: Method,
    rule: &'static Rule,
}

const REGISTRIES: &[&str] = &[
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "rubygems.org",
    "proxy.golang.org",
    "repo.maven.apache.org",
    "repo1.maven.org",
    "hub.docker.com",
    "registry-1.docker.io",
    "ghcr.io",
];
const FORGES: &[&str] = &[
    "github.com",
    "api.github.com",
    "raw.githubusercontent.com",
    "codeload.github.com",
    "gitlab.com",
    "bitbucket.org",
];
const MESSAGING: &[&str] = &[
    "api.sendgrid.com",
    "api.mailgun.net",
    "api.postmarkapp.com",
    "api.resend.com",
    "api.twilio.com",
    "hooks.slack.com",
    "slack.com",
    "discord.com",
    "discordapp.com",
    "api.telegram.org",
];
const DEPLOY: &[&str] = &[
    "api.vercel.com",
    "api.netlify.com",
    "api.cloudflare.com",
    "api.fly.io",
    "api.machines.dev",
    "api.heroku.com",
    "api.render.com",
    "backboard.railway.app",
    "firebasehosting.googleapis.com",
];
const PAYMENT: &[&str] = &["api.stripe.com", "api.paddle.com", "api-m.paypal.com"];
const AUTH: &[&str] = &[
    "oauth2.googleapis.com",
    "login.microsoftonline.com",
    "sts.amazonaws.com",
    "iam.amazonaws.com",
];

/// First match wins, so the narrower method rows come before the general ones.
const ROWS: &[Row] = &[
    Row {
        hosts: REGISTRIES,
        method: Method::Only(&["PUT", "PATCH", "DELETE"]),
        rule: &rule::EGRESS_PUBLISH,
    },
    Row {
        hosts: REGISTRIES,
        method: Method::Read,
        rule: &rule::EGRESS_REGISTRY_READ,
    },
    Row {
        hosts: REGISTRIES,
        method: Method::Write,
        rule: &rule::EGRESS_REGISTRY_WRITE,
    },
    Row {
        hosts: FORGES,
        method: Method::Only(&["DELETE"]),
        rule: &rule::EGRESS_FORGE_DELETE,
    },
    Row {
        hosts: FORGES,
        method: Method::Read,
        rule: &rule::EGRESS_FORGE_READ,
    },
    Row {
        hosts: FORGES,
        method: Method::Write,
        rule: &rule::EGRESS_FORGE_WRITE,
    },
    Row {
        hosts: MESSAGING,
        method: Method::Write,
        rule: &rule::EGRESS_MESSAGING_WRITE,
    },
    Row {
        hosts: MESSAGING,
        method: Method::Read,
        rule: &rule::EGRESS_MESSAGING_READ,
    },
    Row {
        hosts: DEPLOY,
        method: Method::Write,
        rule: &rule::EGRESS_DEPLOY_WRITE,
    },
    Row {
        hosts: DEPLOY,
        method: Method::Read,
        rule: &rule::EGRESS_DEPLOY_READ,
    },
    Row {
        hosts: PAYMENT,
        method: Method::Write,
        rule: &rule::EGRESS_PAYMENT_WRITE,
    },
    Row {
        hosts: PAYMENT,
        method: Method::Read,
        rule: &rule::EGRESS_PAYMENT_READ,
    },
    Row {
        hosts: AUTH,
        method: Method::Write,
        rule: &rule::EGRESS_AUTH_WRITE,
    },
];

pub fn is_read(method: &str) -> bool {
    ["GET", "HEAD", "OPTIONS"]
        .iter()
        .any(|m| method.eq_ignore_ascii_case(m))
}

/// The host as the table compares it: no port, no trailing dot, lower case.
fn normal(host: &str) -> String {
    let host = host.trim().to_lowercase();
    let host = host.rsplit_once(':').map_or(host.as_str(), |(h, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            &host
        }
    });
    host.trim_end_matches('.').to_owned()
}

fn is_host(host: &str, listed: &str) -> bool {
    host == listed
        || host
            .strip_suffix(listed)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// Hosts the builder has approved a request to. Only a decision of the builder counts: a request
/// the classifier let through does not make its host familiar, or an agent could make any host
/// familiar with a harmless read and then send to it.
fn approved(state: &ProjectState, host: &str) -> bool {
    state.actions().any(|a| {
        matches!(&a.action.request, Request::Egress { host: h, .. } if normal(h) == host)
            && a.decided.is_some()
            && matches!(
                a.state,
                ActionState::Approved | ActionState::CarriedOut | ActionState::Failed
            )
    })
}

pub fn read(state: &ProjectState, host: &str, method: &str) -> Reading {
    let host = normal(host);
    let read = is_read(method);
    let hit = ROWS.iter().find(|row| {
        row.hosts.iter().any(|h| is_host(&host, h))
            && match row.method {
                Method::Read => read,
                Method::Write => !read,
                Method::Only(methods) => methods.iter().any(|m| method.eq_ignore_ascii_case(m)),
            }
    });
    let detail = format!("{} {host}", method.to_uppercase());
    if let Some(row) = hit {
        return Reading::Matched(Matched::because(row.rule, detail));
    }
    if approved(state, &host) {
        let rule = if read {
            &rule::EGRESS_SEEN_READ
        } else {
            &rule::EGRESS_SEEN_WRITE
        };
        return Reading::Matched(Matched::because(rule, detail));
    }
    Reading::Unknown {
        floor: if read { Risk::Low } else { Risk::Medium },
    }
}
