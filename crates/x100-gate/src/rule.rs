//! The rule table: every row the gate can match, as data.
//!
//! A row says what an action is, what kind it is, how risky, and the sentence the builder reads
//! when it is held. The code that picks a row lives in `tools`, `command`, `egress` and `merge`;
//! it only chooses among these rows, so reading this file top to bottom is reading the policy.

use std::fmt;

use x100_core::action::{ActionKind, Risk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    /// Stable name, shown to the builder and used by tests.
    pub id: &'static str,
    /// What the row applies to, in plain words.
    pub when: &'static str,
    pub kind: ActionKind,
    pub risk: Risk,
    /// Why it is flagged, in a sentence.
    pub why: &'static str,
}

/// A row that matched, with what in the action made it match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matched {
    pub rule: &'static Rule,
    pub detail: String,
}

impl Matched {
    pub fn of(rule: &'static Rule) -> Self {
        Self {
            rule,
            detail: String::new(),
        }
    }

    pub fn because(rule: &'static Rule, detail: impl Into<String>) -> Self {
        Self {
            rule,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<28} {:<7} {:<20} {}",
            self.id,
            format!("{:?}", self.risk),
            format!("{:?}", self.kind),
            self.when
        )
    }
}

const fn row(
    id: &'static str,
    when: &'static str,
    kind: ActionKind,
    risk: Risk,
    why: &'static str,
) -> Rule {
    Rule {
        id,
        when,
        kind,
        risk,
        why,
    }
}

use ActionKind::*;
use Risk::*;

// Daemon tools. A new tool needs an arm in `tools::tool_rule`, which has no wildcard, and a row here.

pub const ROUTINE: Rule = row(
    "tool.routine",
    "report, wait, update_task, raise_task, review, message, write_spec, add_file, set_preview, \
     load_skill, remember, cancel_job, stop_process, read_log, read_memories, set_up_project, \
     propose_change (a trial)",
    Other,
    Low,
    "Changes the project's own records and nothing outside it.",
);
pub const DELEGATE: Rule = row(
    "tool.delegate",
    "delegate, at any effort below Max",
    Other,
    Low,
    "Gives work to an agent; its budget caps the cost.",
);
pub const DELEGATE_MAX: Rule = row(
    "tool.delegate.max_effort",
    "delegate, or a new worker allowed to reach Max effort",
    Spend,
    Medium,
    "Max effort costs the most per turn.",
);
pub const SCHEDULE_ONCE: Rule = row(
    "tool.schedule.once",
    "schedule, a job that fires once",
    Other,
    Low,
    "Fires once.",
);
pub const SCHEDULE_REPEATING: Rule = row(
    "tool.schedule.repeating",
    "schedule, a job that repeats",
    Spend,
    Medium,
    "A repeating job wakes agents every time it fires, and each wake costs money.",
);
pub const SCHEDULE_FAST: Rule = row(
    "tool.schedule.repeating_fast",
    "schedule, a job that repeats more often than every 10 minutes",
    Spend,
    High,
    "A job that repeats this often can spend a budget in an afternoon.",
);
pub const START_PROCESS: Rule = row(
    "tool.start_process",
    "start_process whose command matches no command row",
    Other,
    Low,
    "Runs in the agent's sandbox, which contains it.",
);
pub const FORM_FIRST: Rule = row(
    "tool.form_team.first",
    "form_team, when the project has no lead yet",
    Other,
    Low,
    "The first team needs no approval.",
);
pub const TEAM_CHANGE: Rule = row(
    "tool.team_change",
    "form_team after the first team; propose_change of a team",
    TeamChange,
    High,
    "Any change to the team after the first is always held, whatever the level.",
);
pub const EFFORT_RAISED: Rule = row(
    "tool.set_effort.raised",
    "set_effort that raises an agent's range",
    Spend,
    Medium,
    "A higher effort range lets the agent's turns cost more.",
);
pub const EFFORT_LOWERED: Rule = row(
    "tool.set_effort.not_raised",
    "set_effort that keeps or lowers the range",
    Other,
    Low,
    "Cannot raise what an agent spends.",
);
pub const BUDGET_RAISED: Rule = row(
    "tool.set_budget.raised",
    "set_budget above the lead's current budget",
    Spend,
    Medium,
    "Raises how much a lead may spend.",
);
pub const BUDGET_OVER_PROJECT: Rule = row(
    "tool.set_budget.over_project",
    "set_budget that takes the leads' budgets past the project's",
    Spend,
    High,
    "The leads could together spend more than the project's budget.",
);
pub const BUDGET_LOWERED: Rule = row(
    "tool.set_budget.not_raised",
    "set_budget that keeps or lowers the lead's budget",
    Other,
    Low,
    "Cannot raise what a lead spends.",
);

// Merges.

pub const MERGE_MAIN: Rule = row(
    "merge.main",
    "the CEO's branch into the project's main branch (a task the builder delegated)",
    Merge,
    Medium,
    "Puts work into the main branch, which is the product.",
);
pub const MERGE_BRANCH: Rule = row(
    "merge.branch",
    "an agent's branch into the delegator's branch",
    Merge,
    Low,
    "Moves work between agents' branches; main is untouched.",
);

// Egress. Rows are tried top to bottom and the first that matches wins; see `egress`.

pub const EGRESS_PUBLISH: Rule = row(
    "egress.registry.publish",
    "PUT, PATCH or DELETE to a package registry",
    SendOutside,
    High,
    "Publishes or changes a package that people outside the project install.",
);
pub const EGRESS_REGISTRY_READ: Rule = row(
    "egress.registry.read",
    "GET, HEAD or OPTIONS to a package registry",
    Other,
    Low,
    "Downloads packages.",
);
pub const EGRESS_REGISTRY_WRITE: Rule = row(
    "egress.registry.write",
    "POST to a package registry",
    Other,
    Medium,
    "Writes to a registry (audit calls look like this, so it is not High).",
);
pub const EGRESS_FORGE_DELETE: Rule = row(
    "egress.forge.delete",
    "DELETE to a code host",
    DeleteData,
    High,
    "Deletes something on a code host.",
);
pub const EGRESS_FORGE_READ: Rule = row(
    "egress.forge.read",
    "GET, HEAD or OPTIONS to a code host",
    Other,
    Low,
    "Reads from a code host.",
);
pub const EGRESS_FORGE_WRITE: Rule = row(
    "egress.forge.write",
    "POST, PUT or PATCH to a code host",
    SendOutside,
    Medium,
    "Pushes or posts to a code host, where others can see it.",
);
pub const EGRESS_MESSAGING_WRITE: Rule = row(
    "egress.messaging.write",
    "a write to a mail or messaging service",
    SendOutside,
    High,
    "Sends a message to people outside the project.",
);
pub const EGRESS_MESSAGING_READ: Rule = row(
    "egress.messaging.read",
    "a read from a mail or messaging service",
    Other,
    Low,
    "Reads from a messaging service.",
);
pub const EGRESS_DEPLOY_WRITE: Rule = row(
    "egress.deploy.write",
    "a write to a hosting or deployment service",
    DeployToProduction,
    High,
    "Changes what is live on a hosting service.",
);
pub const EGRESS_DEPLOY_READ: Rule = row(
    "egress.deploy.read",
    "a read from a hosting or deployment service",
    Other,
    Low,
    "Reads from a hosting service.",
);
pub const EGRESS_PAYMENT_WRITE: Rule = row(
    "egress.payment.write",
    "a write to a payment service",
    Spend,
    High,
    "Moves or commits money.",
);
pub const EGRESS_PAYMENT_READ: Rule = row(
    "egress.payment.read",
    "a read from a payment service",
    Other,
    Medium,
    "Reads customers' and payments' data.",
);
pub const EGRESS_AUTH_WRITE: Rule = row(
    "egress.auth.write",
    "a write to a sign-in or token service",
    ChangeCredentials,
    High,
    "Issues, changes or revokes credentials.",
);
pub const EGRESS_SEEN_READ: Rule = row(
    "egress.seen_host.read",
    "a read from a host the builder has already approved a request to",
    Other,
    Low,
    "A host the project already uses.",
);
pub const EGRESS_SEEN_WRITE: Rule = row(
    "egress.seen_host.write",
    "a write to a host the builder has already approved a request to",
    SendOutside,
    Medium,
    "Sends data to a host the project already uses.",
);

// Commands a harness's permission hook stopped. See `command` for how a command is read.

pub const CMD_ORDINARY: Rule = row(
    "command.ordinary",
    "a command that runs no program from the watched list",
    Other,
    Low,
    "An ordinary command; the sandbox contains it.",
);
pub const CMD_SUDO: Rule = row(
    "command.sudo",
    "sudo, doas or su, anywhere in the command",
    Other,
    High,
    "Runs with other privileges than the agent's own.",
);
pub const CMD_PIPE_TO_SHELL: Rule = row(
    "command.pipe_to_shell",
    "a pipeline that ends in a shell reading it",
    Other,
    High,
    "Runs text it has just fetched or decoded without anyone reading it first.",
);
pub const CMD_FORCE_PUSH: Rule = row(
    "command.force_push",
    "git push with --force, --force-with-lease, -f, --mirror or a +ref",
    ForcePush,
    High,
    "Can overwrite other people's commits on the remote.",
);
pub const CMD_PUSH_DELETE: Rule = row(
    "command.push_delete",
    "git push --delete or :ref",
    DeleteData,
    High,
    "Deletes a branch or tag on the remote.",
);
pub const CMD_REMOTE: Rule = row(
    "command.git_remote",
    "git remote add, set-url, rename or remove; git config of a remote, url or credential",
    ChangeCredentials,
    High,
    "Changes where code is sent or what it is sent with.",
);
pub const CMD_CRED_FILE: Rule = row(
    "command.credential_file",
    "any argument naming a credential file (.ssh, .aws, .netrc, .npmrc, .git-credentials, ...)",
    ChangeCredentials,
    High,
    "Touches a file that holds credentials.",
);
pub const CMD_CRED_TOOL: Rule = row(
    "command.credential_tool",
    "gh auth, aws configure, gcloud auth, npm login or token, docker login, passwd, ssh-add, ...",
    ChangeCredentials,
    High,
    "Signs in, signs out or changes stored credentials.",
);
pub const CMD_DELETE_OUTSIDE: Rule = row(
    "command.delete_outside_workspace",
    "rm, rmdir, unlink, shred or find -delete on a path outside the workspace (absolute, ~, $VAR or ..)",
    DeleteData,
    High,
    "Deletes files outside the agent's own workspace.",
);
pub const CMD_DELETE_REMOTE: Rule = row(
    "command.delete_remote",
    "terraform destroy, kubectl delete, aws s3 rm or rb, gh repo delete, a DROP or TRUNCATE statement, redis FLUSH",
    DeleteData,
    High,
    "Deletes data or infrastructure that lives outside the sandbox.",
);
pub const CMD_DISK: Rule = row(
    "command.disk",
    "mkfs, or dd writing to a device",
    DeleteData,
    High,
    "Overwrites a disk.",
);
pub const CMD_PUBLISH: Rule = row(
    "command.publish",
    "npm, yarn, pnpm, cargo, twine, gem, poetry publish or push; docker push; gh release create; mvn deploy",
    SendOutside,
    High,
    "Publishes code or packages that people outside the project can use.",
);
pub const CMD_DEPLOY_STAGING: Rule = row(
    "command.deploy.non_production",
    "a deploy command that names staging, preview, dev or test",
    DeployToProduction,
    Medium,
    "Deploys, but the command names a non-production target.",
);
pub const CMD_DEPLOY: Rule = row(
    "command.deploy",
    "wrangler, vercel, netlify, fly, firebase, serverless, cdk, helm, terraform apply, kubectl apply, pulumi up, aws or gcloud deploys, ...",
    DeployToProduction,
    High,
    "Changes what is live for users.",
);
pub const CMD_SEND: Rule = row(
    "command.send",
    "mail, sendmail, mutt, msmtp, swaks, aws ses or sns, a Slack or Twilio command line",
    SendOutside,
    High,
    "Sends a message to people outside the project.",
);
pub const CMD_COLLAB: Rule = row(
    "command.collaboration",
    "gh issue or pr create, comment, review, close, edit or merge",
    SendOutside,
    Medium,
    "Posts where other people read it.",
);
pub const CMD_REMOTE_SHELL: Rule = row(
    "command.remote_machine",
    "ssh, scp or sftp",
    Other,
    Medium,
    "Reaches another machine.",
);
pub const CMD_SPEND: Rule = row(
    "command.spend",
    "creating cloud machines, databases or clusters (aws ec2, rds, eks; gcloud compute; az vm)",
    Spend,
    High,
    "Starts paid infrastructure.",
);

// What the rules do not cover.

pub const UNCLASSIFIED_NOTHING: Rule = row(
    "classifier.skipped",
    "no row covers it, and \"Ask me about\" is Nothing",
    Other,
    Low,
    "No rule covers it, and the builder asked to be asked about nothing, so the classifier was not called.",
);

/// Every row, in the order a person reads them. A test checks that no id repeats and that every
/// ruling the gate gives names a row from here.
pub const ALL: &[Rule] = &[
    ROUTINE,
    DELEGATE,
    DELEGATE_MAX,
    SCHEDULE_ONCE,
    SCHEDULE_REPEATING,
    SCHEDULE_FAST,
    START_PROCESS,
    FORM_FIRST,
    TEAM_CHANGE,
    EFFORT_RAISED,
    EFFORT_LOWERED,
    BUDGET_RAISED,
    BUDGET_OVER_PROJECT,
    BUDGET_LOWERED,
    MERGE_MAIN,
    MERGE_BRANCH,
    EGRESS_PUBLISH,
    EGRESS_REGISTRY_READ,
    EGRESS_REGISTRY_WRITE,
    EGRESS_FORGE_DELETE,
    EGRESS_FORGE_READ,
    EGRESS_FORGE_WRITE,
    EGRESS_MESSAGING_WRITE,
    EGRESS_MESSAGING_READ,
    EGRESS_DEPLOY_WRITE,
    EGRESS_DEPLOY_READ,
    EGRESS_PAYMENT_WRITE,
    EGRESS_PAYMENT_READ,
    EGRESS_AUTH_WRITE,
    EGRESS_SEEN_READ,
    EGRESS_SEEN_WRITE,
    CMD_ORDINARY,
    CMD_SUDO,
    CMD_PIPE_TO_SHELL,
    CMD_FORCE_PUSH,
    CMD_PUSH_DELETE,
    CMD_REMOTE,
    CMD_CRED_FILE,
    CMD_CRED_TOOL,
    CMD_DELETE_OUTSIDE,
    CMD_DELETE_REMOTE,
    CMD_DISK,
    CMD_PUBLISH,
    CMD_DEPLOY_STAGING,
    CMD_DEPLOY,
    CMD_SEND,
    CMD_COLLAB,
    CMD_REMOTE_SHELL,
    CMD_SPEND,
    UNCLASSIFIED_NOTHING,
];
