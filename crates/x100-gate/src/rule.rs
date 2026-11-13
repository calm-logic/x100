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

/// Declares each row as a constant and lists every row in [`ALL`], in the order written, so a
/// row is added in one place.
macro_rules! table {
    ($($name:ident = $row:expr;)*) => {
        $(pub const $name: Rule = $row;)*

        /// Every row, in the order a person reads them. A test checks that no id repeats and that
        /// every ruling the gate gives names a row from here.
        pub const ALL: &[Rule] = &[$($name),*];
    };
}

table! {
    // Daemon tools. A new tool, such as a deploy through an integration, gets its row here, with
    // its kind and risk, and an arm in `tools::rule` naming that row. That match has no wildcard,
    // so the tool does not compile until it has one.

    ROUTINE = row(
        "tool.routine",
        "report, wait, update_task, raise_task, review, message, write_spec, add_file, set_preview, \
         load_skill, remember, cancel_job, stop_process, read_log, read_memories, set_up_project, \
         propose_change (a trial)",
        Other,
        Low,
        "Changes the project's own records and nothing outside it.",
    );
    DELEGATE = row(
        "tool.delegate",
        "delegate, at any effort below Max",
        Other,
        Low,
        "Gives work to an agent; its budget caps the cost.",
    );
    DELEGATE_MAX = row(
        "tool.delegate.max_effort",
        "delegate, or a new worker allowed to reach Max effort",
        Spend,
        Medium,
        "Max effort costs the most per turn.",
    );
    SCHEDULE_ONCE = row(
        "tool.schedule.once",
        "schedule, a job that fires once",
        Other,
        Low,
        "Fires once.",
    );
    SCHEDULE_REPEATING = row(
        "tool.schedule.repeating",
        "schedule, a job that repeats",
        Spend,
        Medium,
        "A repeating job wakes agents every time it fires, and each wake costs money.",
    );
    SCHEDULE_FAST = row(
        "tool.schedule.repeating_fast",
        "schedule, a job that repeats more often than every 10 minutes",
        Spend,
        High,
        "A job that repeats this often can spend a budget in an afternoon.",
    );
    START_PROCESS = row(
        "tool.start_process",
        "start_process whose command matches no command row",
        Other,
        Low,
        "Runs in the agent's sandbox, which contains it.",
    );
    FORM_FIRST = row(
        "tool.form_team.first",
        "form_team, when the project has no lead yet",
        Other,
        Low,
        "The first team needs no approval.",
    );
    TEAM_CHANGE = row(
        "tool.team_change",
        "form_team after the first team; propose_change of a team",
        TeamChange,
        High,
        "Any change to the team after the first is always held, whatever the level.",
    );
    EFFORT_RAISED = row(
        "tool.set_effort.raised",
        "set_effort that raises an agent's range",
        Spend,
        Medium,
        "A higher effort range lets the agent's turns cost more.",
    );
    EFFORT_LOWERED = row(
        "tool.set_effort.not_raised",
        "set_effort that keeps or lowers the range",
        Other,
        Low,
        "Cannot raise what an agent spends.",
    );
    BUDGET_RAISED = row(
        "tool.set_budget.raised",
        "set_budget above the lead's current budget",
        Spend,
        Medium,
        "Raises how much a lead may spend.",
    );
    BUDGET_OVER_PROJECT = row(
        "tool.set_budget.over_project",
        "set_budget that takes the leads' budgets past the project's",
        Spend,
        High,
        "The leads could together spend more than the project's budget.",
    );
    BUDGET_LOWERED = row(
        "tool.set_budget.not_raised",
        "set_budget that keeps or lowers the lead's budget",
        Other,
        Low,
        "Cannot raise what a lead spends.",
    );

    // Integrations. Each call is the daemon's own, with a key no agent holds.

    LIST_INTEGRATIONS = row(
        "tool.list_integrations",
        "list_integrations",
        Other,
        Low,
        "Reads what is connected and what the project could be linked to; changes nothing.",
    );
    LINK_PROJECT = row(
        "tool.link_project",
        "link_project",
        Other,
        Low,
        "Changes which Homebox project this project deploys to; nothing is deployed or sent.",
    );
    DEPLOY = row(
        "tool.deploy",
        "deploy, of a project already on GitHub: push main and deploy it through Homebox",
        DeployToProduction,
        High,
        "Pushes the code and changes what is live at a public address.",
    );
    DEPLOY_PUBLISH = row(
        "tool.deploy.publish_first",
        "deploy, of a project not on GitHub yet: create a private repository, push main, add it to Homebox, link and deploy",
        DeployToProduction,
        High,
        "Creates a repository, pushes the code off this machine and makes it live at a public address.",
    );

    // Merges.

    MERGE_MAIN = row(
        "merge.main",
        "the CEO's branch into the project's main branch (a task the builder delegated)",
        Merge,
        Medium,
        "Puts work into the main branch, which is the product.",
    );
    MERGE_BRANCH = row(
        "merge.branch",
        "an agent's branch into the delegator's branch",
        Merge,
        Low,
        "Moves work between agents' branches; main is untouched.",
    );

    // Egress. Rows are tried top to bottom and the first that matches wins; see `egress`.

    EGRESS_PUBLISH = row(
        "egress.registry.publish",
        "PUT, PATCH or DELETE to a package registry",
        SendOutside,
        High,
        "Publishes or changes a package that people outside the project install.",
    );
    EGRESS_REGISTRY_READ = row(
        "egress.registry.read",
        "GET, HEAD or OPTIONS to a package registry",
        Other,
        Low,
        "Downloads packages.",
    );
    EGRESS_REGISTRY_WRITE = row(
        "egress.registry.write",
        "POST to a package registry",
        Other,
        Medium,
        "Writes to a registry (audit calls look like this, so it is not High).",
    );
    EGRESS_FORGE_DELETE = row(
        "egress.forge.delete",
        "DELETE to a code host",
        DeleteData,
        High,
        "Deletes something on a code host.",
    );
    EGRESS_FORGE_READ = row(
        "egress.forge.read",
        "GET, HEAD or OPTIONS to a code host",
        Other,
        Low,
        "Reads from a code host.",
    );
    EGRESS_FORGE_WRITE = row(
        "egress.forge.write",
        "POST, PUT or PATCH to a code host",
        SendOutside,
        Medium,
        "Pushes or posts to a code host, where others can see it.",
    );
    EGRESS_MESSAGING_WRITE = row(
        "egress.messaging.write",
        "a write to a mail or messaging service",
        SendOutside,
        High,
        "Sends a message to people outside the project.",
    );
    EGRESS_MESSAGING_READ = row(
        "egress.messaging.read",
        "a read from a mail or messaging service",
        Other,
        Low,
        "Reads from a messaging service.",
    );
    EGRESS_DEPLOY_WRITE = row(
        "egress.deploy.write",
        "a write to a hosting or deployment service",
        DeployToProduction,
        High,
        "Changes what is live on a hosting service.",
    );
    EGRESS_DEPLOY_READ = row(
        "egress.deploy.read",
        "a read from a hosting or deployment service",
        Other,
        Low,
        "Reads from a hosting service.",
    );
    EGRESS_PAYMENT_WRITE = row(
        "egress.payment.write",
        "a write to a payment service",
        Spend,
        High,
        "Moves or commits money.",
    );
    EGRESS_PAYMENT_READ = row(
        "egress.payment.read",
        "a read from a payment service",
        Other,
        Medium,
        "Reads customers' and payments' data.",
    );
    EGRESS_AUTH_WRITE = row(
        "egress.auth.write",
        "a write to a sign-in or token service",
        ChangeCredentials,
        High,
        "Issues, changes or revokes credentials.",
    );
    EGRESS_SEEN_READ = row(
        "egress.seen_host.read",
        "a read from a host the builder has already approved a request to",
        Other,
        Low,
        "A host the project already uses.",
    );
    EGRESS_SEEN_WRITE = row(
        "egress.seen_host.write",
        "a write to a host the builder has already approved a request to",
        SendOutside,
        Medium,
        "Sends data to a host the project already uses.",
    );

    // Egress the gate does not judge, because x100 itself decided it. Recorded all the same.
    EGRESS_PROVIDER = row(
        "egress.own_provider",
        "a request from a driver's harness to the model provider x100 started it on",
        Other,
        Low,
        "The harness's own traffic to its model, which x100 chose; not the agent's action.",
    );
    EGRESS_REFUSED = row(
        "egress.refused_by_proxy",
        "a host the proxy refuses whatever the gate says: the host machine's loopback, or one refused by name",
        Other,
        Low,
        "Nothing went out: the proxy refuses this host.",
    );

    // Commands a harness's permission hook stopped. See `command` for how a command is read.

    CMD_ORDINARY = row(
        "command.ordinary",
        "a command that runs no program from the watched list",
        Other,
        Low,
        "An ordinary command; the sandbox contains it.",
    );
    CMD_SUDO = row(
        "command.sudo",
        "sudo, doas or su, anywhere in the command",
        Other,
        High,
        "Runs with other privileges than the agent's own.",
    );
    CMD_PIPE_TO_SHELL = row(
        "command.pipe_to_shell",
        "a pipeline that ends in a shell reading it",
        Other,
        High,
        "Runs text it has just fetched or decoded without anyone reading it first.",
    );
    CMD_FORCE_PUSH = row(
        "command.force_push",
        "git push with --force, --force-with-lease, -f, --mirror or a +ref",
        ForcePush,
        High,
        "Can overwrite other people's commits on the remote.",
    );
    CMD_PUSH_DELETE = row(
        "command.push_delete",
        "git push --delete or :ref",
        DeleteData,
        High,
        "Deletes a branch or tag on the remote.",
    );
    CMD_REMOTE = row(
        "command.git_remote",
        "git remote add, set-url, rename or remove; git config of a remote, url or credential",
        ChangeCredentials,
        High,
        "Changes where code is sent or what it is sent with.",
    );
    CMD_CRED_FILE = row(
        "command.credential_file",
        "any argument naming a credential file (.ssh, .aws, .netrc, .npmrc, .git-credentials, ...)",
        ChangeCredentials,
        High,
        "Touches a file that holds credentials.",
    );
    CMD_CRED_TOOL = row(
        "command.credential_tool",
        "gh auth, aws configure, gcloud auth, npm login or token, docker login, passwd, ssh-add, ...",
        ChangeCredentials,
        High,
        "Signs in, signs out or changes stored credentials.",
    );
    CMD_DELETE_OUTSIDE = row(
        "command.delete_outside_workspace",
        "rm, rmdir, unlink, shred or find -delete on a path outside the workspace (absolute, ~, $VAR or ..)",
        DeleteData,
        High,
        "Deletes files outside the agent's own workspace.",
    );
    CMD_DELETE_REMOTE = row(
        "command.delete_remote",
        "terraform destroy, kubectl delete, aws s3 rm or rb, gh repo delete, a DROP or TRUNCATE statement, redis FLUSH",
        DeleteData,
        High,
        "Deletes data or infrastructure that lives outside the sandbox.",
    );
    CMD_DISK = row(
        "command.disk",
        "mkfs, or dd writing to a device",
        DeleteData,
        High,
        "Overwrites a disk.",
    );
    CMD_PUBLISH = row(
        "command.publish",
        "npm, yarn, pnpm, cargo, twine, gem, poetry publish or push; docker push; gh release create; mvn deploy",
        SendOutside,
        High,
        "Publishes code or packages that people outside the project can use.",
    );
    CMD_DEPLOY_STAGING = row(
        "command.deploy.non_production",
        "a deploy command that names staging, preview, dev or test",
        DeployToProduction,
        Medium,
        "Deploys, but the command names a non-production target.",
    );
    CMD_DEPLOY = row(
        "command.deploy",
        "wrangler, vercel, netlify, fly, firebase, serverless, cdk, helm, terraform apply, kubectl apply, pulumi up, aws or gcloud deploys, ...",
        DeployToProduction,
        High,
        "Changes what is live for users.",
    );
    CMD_SEND = row(
        "command.send",
        "mail, sendmail, mutt, msmtp, swaks, aws ses or sns, a Slack or Twilio command line",
        SendOutside,
        High,
        "Sends a message to people outside the project.",
    );
    CMD_COLLAB = row(
        "command.collaboration",
        "gh issue or pr create, comment, review, close, edit or merge",
        SendOutside,
        Medium,
        "Posts where other people read it.",
    );
    CMD_REMOTE_SHELL = row(
        "command.remote_machine",
        "ssh, scp or sftp",
        Other,
        Medium,
        "Reaches another machine.",
    );
    CMD_SPEND = row(
        "command.spend",
        "creating cloud machines, databases or clusters (aws ec2, rds, eks; gcloud compute; az vm)",
        Spend,
        High,
        "Starts paid infrastructure.",
    );

    // What the rules do not cover.

    UNCLASSIFIED_NOTHING = row(
        "classifier.skipped",
        "no row covers it, and \"Ask me about\" is Nothing",
        Other,
        Low,
        "No rule covers it, and the builder asked to be asked about nothing, so the classifier was not called.",
    );
    NOT_CONNECTED_READ = row(
        "classifier.not_connected.read",
        "no row covers it, it only reads, and no decision model is connected",
        Other,
        Low,
        "No rule covers it and no decision model is connected to judge it. It only reads, so it goes at the lowest risk a read can have.",
    );
    NOT_CONNECTED_WRITE = row(
        "classifier.not_connected.write",
        "no row covers it, it may change something, and no decision model is connected",
        Other,
        High,
        "No rule covers it and no decision model is connected to judge it, so it waits for the builder. Connect one in Settings to have such actions judged.",
    );
}
