//! Wording for actions: what a request is, and each step an action goes through.

use x100_core::action::{ActionEvent, ActionKind, ClassifiedBy, Request};
use x100_core::id::ActionId;
use x100_core::task::Verdict;
use x100_core::tool::{Delegated, Recipient, ToolCall};

use super::{Names, quote, snippet};
use crate::words;

/// What an action is, as a phrase that follows "wants to".
pub fn request(n: &Names, r: &Request) -> String {
    match r {
        Request::Tool { call, .. } => tool(n, call),
        Request::Command { command, .. } => format!("run {command}"),
        Request::Egress { host, method, .. } => format!("send a {method} request to {host}"),
        Request::Merge { task } => format!("merge the work on {}", n.task(*task)),
    }
}

fn tool(n: &Names, call: &ToolCall) -> String {
    match call {
        ToolCall::Delegate { task, .. } => match task {
            Delegated::New { title, .. } => format!("delegate {}", quote(title)),
            Delegated::Raised(id) => format!("delegate {}", n.task(*id)),
        },
        ToolCall::Review { task, verdict } => format!(
            "{} the work on {}",
            match verdict {
                Verdict::Accept => "accept",
                Verdict::SendBack { .. } => "send back",
            },
            n.task(*task)
        ),
        ToolCall::Report { task, .. } => format!("report on {}", n.task(*task)),
        ToolCall::Message { to, .. } => match to {
            Recipient::Agent(a) => format!("message {}", n.agent(*a)),
            Recipient::Builder => "message the builder".into(),
        },
        ToolCall::Wait { .. } => "wait".into(),
        ToolCall::Schedule { .. } => "schedule a job".into(),
        ToolCall::CancelJob { job } => format!("cancel {}", n.job(*job)),
        ToolCall::RaiseTask { title, .. } => format!("raise the task {}", quote(title)),
        ToolCall::UpdateTask { task, .. } => format!("update {}", n.task(*task)),
        ToolCall::WriteSpec { spec, .. } => {
            format!("write the {}", words::spec_name(*spec).to_lowercase())
        }
        ToolCall::AddFile { name, .. } => format!("add the file {name}"),
        ToolCall::SetPreview { url } => format!("set the preview address to {url}"),
        ToolCall::SetUpProject { name, .. } => format!("set the project up as {}", quote(name)),
        ToolCall::ListIntegrations {} => "list the integrations".into(),
        ToolCall::LinkProject {
            homebox_project, ..
        } => format!(
            "link the project to the Homebox project {}",
            quote(homebox_project)
        ),
        ToolCall::Deploy { environment } => match environment {
            Some(env) => format!("deploy the project through Homebox to {env}"),
            None => "deploy the project through Homebox".into(),
        },
        ToolCall::LoadSkill { skill } => format!("load the skill {}", skill.name.0),
        ToolCall::StartProcess { command, .. } => format!("start {command}"),
        ToolCall::StopProcess { name } => format!("stop {}", name.0),
        ToolCall::Remember { note, .. } => format!("write the memory note {}", note.0),
        ToolCall::FormTeam { leads } => format!("form a team of {} leads", leads.len()),
        ToolCall::SetEffort { agent, .. } => format!("change {}'s effort range", n.agent(*agent)),
        ToolCall::SetBudget { lead, budget } => {
            format!(
                "set {}'s budget to {}",
                n.agent(*lead),
                words::money(*budget)
            )
        }
        ToolCall::ReadLog { .. } => "read the log".into(),
        ToolCall::ReadMemories { agent } => format!("read {}'s memories", n.agent(*agent)),
        ToolCall::ProposeChange(_) => "propose a change to the team or its skills".into(),
    }
}

pub(super) fn action_event(n: &Names, action: ActionId, event: &ActionEvent) -> String {
    use ActionEvent as A;
    let what = n.action(action);
    match event {
        A::Classified(c) => format!(
            "Rated as {} ({}), by {}: {what}. {}",
            kind(c.kind),
            words::risk(c.risk).to_lowercase(),
            match c.by {
                ClassifiedBy::Rules => "rule",
                ClassifiedBy::Classifier(_) => "classifier",
            },
            c.why
        ),
        A::Allowed => format!("Let through: {what}"),
        A::Held => format!("Held for the builder: {what}"),
        A::Approved => format!("The builder approved: {what}"),
        A::Denied => format!("The builder denied: {what}"),
        A::CarriedOut => format!("Carried out: {what}"),
        A::Failed { error } => format!("Failed: {what}. {}", snippet(error)),
    }
}

pub fn kind(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::DeleteData => "deleting data",
        ActionKind::DeployToProduction => "a deploy to production",
        ActionKind::Spend => "spending",
        ActionKind::SendOutside => "sending something outside",
        ActionKind::ChangeCredentials => "a credentials change",
        ActionKind::ForcePush => "a force push",
        ActionKind::TeamChange => "a team change",
        ActionKind::Merge => "a merge",
        ActionKind::Other => "something else",
    }
}
