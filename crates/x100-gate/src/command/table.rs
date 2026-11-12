//! The deny-list as data: the patterns a command is matched against, and the programs that
//! decide whether the hook asks and whether the classifier is consulted.

use crate::rule::{self, Rule};

pub(super) struct Pattern {
    pub(super) rule: &'static Rule,
    pub(super) programs: &'static [&'static str],
    pub(super) sub: &'static [&'static str],
    pub(super) with: &'static [&'static str],
    pub(super) prefix: &'static [&'static str],
    pub(super) contains: &'static [&'static str],
    /// A softer row for the same command when it names a target that is not production.
    pub(super) soften: Option<&'static Rule>,
}

pub(super) const fn pat(rule: &'static Rule, programs: &'static [&'static str]) -> Pattern {
    Pattern {
        rule,
        programs,
        sub: &[],
        with: &[],
        prefix: &[],
        contains: &[],
        soften: None,
    }
}

impl Pattern {
    const fn sub(mut self, sub: &'static [&'static str]) -> Self {
        self.sub = sub;
        self
    }
    const fn with(mut self, with: &'static [&'static str]) -> Self {
        self.with = with;
        self
    }
    const fn prefix(mut self, prefix: &'static [&'static str]) -> Self {
        self.prefix = prefix;
        self
    }
    const fn contains(mut self, contains: &'static [&'static str]) -> Self {
        self.contains = contains;
        self
    }
    const fn soften(mut self) -> Self {
        self.soften = Some(&rule::CMD_DEPLOY_STAGING);
        self
    }
}

use rule::*;

const SQL_CLIENTS: &[&str] = &[
    "psql",
    "mysql",
    "mariadb",
    "sqlite3",
    "mongosh",
    "mongo",
    "redis-cli",
    "cockroach",
];

/// The deny-list, as data. When several rows match one command the riskiest wins, and the first
/// of equals.
pub(super) const PATTERNS: &[Pattern] = &[
    pat(&CMD_FORCE_PUSH, &["git"])
        .sub(&["push"])
        .with(&[
            "--force",
            "--force-with-lease",
            "--force-if-includes",
            "-f",
            "--mirror",
        ])
        .prefix(&["+"]),
    pat(&CMD_PUSH_DELETE, &["git"])
        .sub(&["push"])
        .with(&["--delete", "-d"])
        .prefix(&[":"]),
    pat(&CMD_REMOTE, &["git"])
        .sub(&["remote"])
        .with(&["add", "set-url", "rename", "remove", "rm"]),
    pat(&CMD_REMOTE, &["git"]).sub(&["config"]).contains(&[
        "remote.",
        "credential",
        "url.",
        "insteadof",
    ]),
    pat(&CMD_CRED_TOOL, &["git"]).sub(&["credential"]),
    pat(&CMD_CRED_TOOL, &["gh"]).sub(&["auth"]),
    pat(&CMD_CRED_TOOL, &["aws"]).sub(&["configure", "sso"]),
    pat(&CMD_CRED_TOOL, &["gcloud"]).sub(&["auth"]),
    pat(&CMD_CRED_TOOL, &["az"]).sub(&["login", "logout"]),
    pat(&CMD_CRED_TOOL, &["npm", "pnpm", "yarn"])
        .sub(&["login", "adduser", "logout", "token", "owner", "access"]),
    pat(&CMD_CRED_TOOL, &["cargo"]).sub(&["login", "logout", "owner"]),
    pat(&CMD_CRED_TOOL, &["docker", "podman"]).sub(&["login", "logout"]),
    pat(&CMD_CRED_TOOL, &["kubectl"]).sub(&["config"]),
    pat(
        &CMD_CRED_TOOL,
        &[
            "heroku", "flyctl", "fly", "vercel", "netlify", "wrangler", "firebase",
        ],
    )
    .sub(&["login", "logout", "auth", "tokens", "secrets", "secret"]),
    pat(
        &CMD_CRED_TOOL,
        &[
            "passwd",
            "chpasswd",
            "ssh-add",
            "ssh-keygen",
            "htpasswd",
            "security",
            "usermod",
        ],
    ),
    pat(&CMD_DELETE_REMOTE, &["terraform", "tofu"]).sub(&["destroy"]),
    pat(&CMD_DELETE_REMOTE, &["kubectl"]).sub(&["delete"]),
    pat(&CMD_DELETE_REMOTE, &["helm"]).sub(&["uninstall", "delete"]),
    pat(&CMD_DELETE_REMOTE, &["cdk"]).sub(&["destroy"]),
    pat(&CMD_DELETE_REMOTE, &["gh"])
        .sub(&["repo"])
        .with(&["delete"]),
    pat(&CMD_DELETE_REMOTE, &["aws"])
        .with(&["rm", "rb"])
        .prefix(&["delete-", "terminate-", "deregister-"]),
    pat(&CMD_DELETE_REMOTE, &["gcloud", "az", "doctl"]).with(&["delete"]),
    pat(&CMD_DELETE_REMOTE, SQL_CLIENTS).contains(&[
        "drop table",
        "drop database",
        "drop schema",
        "truncate ",
        "delete from",
        "flushall",
        "flushdb",
    ]),
    pat(&CMD_DISK, &["mkfs", "wipefs", "fdisk", "parted"]),
    pat(&CMD_DISK, &["dd"]).contains(&["of=/dev/"]),
    pat(&CMD_PUBLISH, &["npm", "pnpm", "yarn"]).sub(&["publish", "unpublish", "deprecate"]),
    pat(&CMD_PUBLISH, &["cargo"]).sub(&["publish", "yank"]),
    pat(&CMD_PUBLISH, &["twine"]).sub(&["upload"]),
    pat(&CMD_PUBLISH, &["gem"]).sub(&["push", "yank"]),
    pat(&CMD_PUBLISH, &["poetry", "flit", "vsce", "ovsx"]).sub(&["publish"]),
    pat(&CMD_PUBLISH, &["docker", "podman", "helm"]).sub(&["push"]),
    pat(&CMD_PUBLISH, &["gh"])
        .sub(&["release"])
        .with(&["create", "upload"]),
    pat(&CMD_PUBLISH, &["mvn"]).sub(&["deploy"]),
    pat(&CMD_PUBLISH, &["dotnet"])
        .sub(&["nuget"])
        .with(&["push"]),
    pat(&CMD_DEPLOY, &["wrangler"])
        .sub(&["deploy", "publish", "pages"])
        .soften(),
    pat(&CMD_DEPLOY, &["vercel"])
        .with(&["--prod", "--production", "deploy", "promote"])
        .soften(),
    pat(
        &CMD_DEPLOY,
        &[
            "netlify",
            "firebase",
            "serverless",
            "sls",
            "cdk",
            "railway",
            "render",
        ],
    )
    .sub(&["deploy", "up"])
    .soften(),
    pat(&CMD_DEPLOY, &["fly", "flyctl"])
        .sub(&["deploy", "launch", "scale"])
        .soften(),
    pat(&CMD_DEPLOY, &["helm"])
        .sub(&["install", "upgrade", "rollback"])
        .soften(),
    pat(&CMD_DEPLOY, &["terraform", "tofu"])
        .sub(&["apply"])
        .soften(),
    pat(&CMD_DEPLOY, &["pulumi"]).sub(&["up"]).soften(),
    pat(&CMD_DEPLOY, &["kubectl"])
        .sub(&[
            "apply", "create", "replace", "patch", "scale", "rollout", "set", "run", "expose",
        ])
        .soften(),
    pat(&CMD_DEPLOY, &["gcloud"]).with(&["deploy"]).soften(),
    pat(&CMD_DEPLOY, &["aws"])
        .with(&[
            "deploy",
            "update-function-code",
            "update-service",
            "create-deployment",
        ])
        .soften(),
    pat(&CMD_DEPLOY, &["ansible-playbook"]).soften(),
    pat(&CMD_DEPLOY, &["make"])
        .with(&["deploy", "release", "publish"])
        .soften(),
    pat(&CMD_DEPLOY, &["yarn", "pnpm", "bun"])
        .sub(&["deploy"])
        .soften(),
    pat(&CMD_DEPLOY, &["npm", "pnpm", "yarn", "bun"])
        .sub(&["run", "run-script"])
        .prefix(&["deploy", "release", "publish"])
        .soften(),
    pat(
        &CMD_SEND,
        &[
            "mail", "mailx", "sendmail", "mutt", "msmtp", "swaks", "mpack", "slackcat", "twilio",
        ],
    ),
    pat(&CMD_SEND, &["aws"]).sub(&["ses", "sesv2", "sns", "pinpoint"]),
    pat(&CMD_COLLAB, &["gh"]).sub(&["issue", "pr"]).with(&[
        "create", "comment", "review", "close", "edit", "merge", "reopen",
    ]),
    pat(&CMD_REMOTE_SHELL, &["ssh", "scp", "sftp"]),
    pat(&CMD_SPEND, &["aws"])
        .sub(&["ec2", "rds", "eks", "redshift", "sagemaker", "elasticache"])
        .prefix(&["run-", "create-", "start-"]),
    pat(&CMD_SPEND, &["gcloud"])
        .sub(&["compute", "container", "sql"])
        .with(&["create"]),
    pat(&CMD_SPEND, &["az"])
        .sub(&["vm", "aks", "sql"])
        .with(&["create"]),
    pat(&CMD_SPEND, &["doctl"]).with(&["create"]),
    pat(&CMD_SPEND, &["stripe"]).with(&["create", "charge", "refund", "payout", "transfers"]),
];

/// Programs the hook should stop that no pattern names: the readers of deletes and of pipes into
/// a shell, and the elevators, which `simple` reports on its own.
pub(super) const WATCHED_BESIDES: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "find", "eval", "sh", "bash", "zsh", "dash", "ksh", "fish",
    "sudo", "doas", "su", "pkexec",
];
/// Programs with no safe default use: reaching a cloud, a database or a payment account. When one
/// runs and no pattern matched, the classifier is asked. Programs not here and not matched, such
/// as `git status` or `cargo test`, are ordinary.
pub(super) const ASK_WHEN_UNMATCHED: &[&str] = &[
    "aws",
    "gcloud",
    "az",
    "kubectl",
    "helm",
    "terraform",
    "tofu",
    "pulumi",
    "wrangler",
    "vercel",
    "netlify",
    "fly",
    "flyctl",
    "firebase",
    "serverless",
    "sls",
    "cdk",
    "heroku",
    "railway",
    "render",
    "doctl",
    "gh",
    "stripe",
    "psql",
    "mysql",
    "mariadb",
    "sqlite3",
    "mongosh",
    "mongo",
    "redis-cli",
    "cockroach",
    "ansible-playbook",
    "eval",
];
