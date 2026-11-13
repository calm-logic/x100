//! Deploying a commit that is already pushed, by Homebox's rule.
//!
//! With `auto_deploy` on, the push itself deploys by webhook, so calling deploy as well would
//! run it twice: x100 waits for the deployment of the pushed commit. With it off, x100 calls
//! deploy, which takes the branch head, and checks the commit it deployed. A deployment that is
//! running may still be a container that keeps crashing, so the environment's instances must
//! answer too. A failure says why, with Homebox's error and the end of its log.

use std::time::Duration;

use x100_core::integration::HomeboxLink;

use super::{Deployment, Environment, Homebox, Project};
use crate::api::{ApiError, short};

/// How often Homebox is asked, and how long anything may take.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// The first wait between two polls, doubled up to `most`.
    pub first: Duration,
    pub most: Duration,
    /// How long the webhook may take to start a deployment of a pushed commit.
    pub appear_within: Duration,
    /// How long a deployment may take, from queued to running.
    pub finish_within: Duration,
    /// How many polls a running deployment's instances get to answer.
    pub settle_polls: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            first: Duration::from_millis(250),
            most: Duration::from_secs(3),
            appear_within: Duration::from_secs(180),
            finish_within: Duration::from_secs(15 * 60),
            settle_polls: 5,
        }
    }
}

/// A deploy that is live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployed {
    /// The public address, when the project has a public service.
    pub address: Option<String>,
    pub commit: String,
    pub deployment: String,
}

/// How the deployment to watch is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Watch {
    /// Homebox already queued it, as adding a project does.
    Queued(String),
    /// Follow the project's `auto_deploy` rule for `commit`, just pushed.
    Rule,
}

struct Backoff {
    next: Duration,
    most: Duration,
}

impl Backoff {
    fn new(t: &Timing) -> Self {
        Self {
            next: t.first,
            most: t.most,
        }
    }

    async fn wait(&mut self) {
        tokio::time::sleep(self.next).await;
        self.next = (self.next * 2).min(self.most);
    }
}

fn says(e: ApiError) -> String {
    e.why
}

/// The linked project as it is now: found by its id, or by its repository if the id changed. A
/// project that is gone or no longer managed refuses deploys until the builder links again.
pub async fn current(hb: &Homebox<'_>, link: &HomeboxLink) -> Result<Project, String> {
    let project = match hb.project(&link.project_id).await {
        Ok(p) if p.repo_full_name.eq_ignore_ascii_case(&link.repo_full_name) => Some(p),
        Ok(_)
        | Err(ApiError {
            status: Some(404), ..
        }) => None,
        Err(e) => return Err(e.why),
    };
    let project = match project {
        Some(p) => p,
        None => {
            let all = hb.projects().await.map_err(says)?;
            let found = all
                .into_iter()
                .find(|p| p.repo_full_name.eq_ignore_ascii_case(&link.repo_full_name))
                .ok_or_else(|| {
                    format!(
                        "Homebox no longer has the project for {}: link the project again",
                        link.repo_full_name
                    )
                })?;
            hb.project(&found.id).await.map_err(says)?
        }
    };
    if !project.managed {
        return Err(format!(
            "Homebox no longer manages {} (it was released): add it in Homebox or link another \
             project",
            project.repo_full_name
        ));
    }
    Ok(project)
}

fn environment<'p>(project: &'p Project, name: &str) -> Result<&'p Environment, String> {
    project
        .environments
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let names: Vec<&str> = project
                .environments
                .iter()
                .map(|e| e.name.as_str())
                .collect();
            format!(
                "the Homebox project {} has no environment {name}; it has: {}",
                project.name,
                names.join(", ")
            )
        })
}

/// Deploys `commit`, already pushed to the linked repository, and waits until it is live or has
/// failed.
pub async fn deploy(
    hb: &Homebox<'_>,
    link: &HomeboxLink,
    commit: &str,
    watch: Watch,
    timing: &Timing,
) -> Result<Deployed, String> {
    let project = current(hb, link).await?;
    let env = environment(&project, &link.environment)?.clone();
    let id = match watch {
        Watch::Queued(id) => id,
        Watch::Rule if project.auto_deploy => match &env.deployment {
            // Nothing new was pushed, so no webhook fires: what is live is already this commit.
            Some(d) if d.status == "running" && d.commit_sha.as_deref() == Some(commit) => {
                d.id.clone()
            }
            _ => pushed(hb, &project, &env, commit, timing).await?,
        },
        Watch::Rule => hb.deploy(&project.id, &env.id).await.map_err(says)?,
    };
    let done = finished(hb, &project, &id, timing).await?;
    if done.commit_sha.as_deref() != Some(commit) {
        return Err(format!(
            "Homebox deployed {}, not the pushed commit {commit}: the branch moved meanwhile",
            done.commit_sha.as_deref().unwrap_or("an unknown commit")
        ));
    }
    let address = answering(hb, &project, &env.name, &id, timing).await?;
    Ok(Deployed {
        address,
        commit: commit.to_owned(),
        deployment: id,
    })
}

/// The deployment the webhook started for `commit`. Its commit is known only once Homebox has
/// cloned it, so the list is asked until one names it.
async fn pushed(
    hb: &Homebox<'_>,
    project: &Project,
    env: &Environment,
    commit: &str,
    timing: &Timing,
) -> Result<String, String> {
    let until = tokio::time::Instant::now() + timing.appear_within;
    let mut backoff = Backoff::new(timing);
    loop {
        let listed = hb.deployments(&project.id, &env.id).await.map_err(says)?;
        if let Some(d) = listed
            .iter()
            .find(|d| d.commit_sha.as_deref() == Some(commit))
        {
            return Ok(d.id.clone());
        }
        if tokio::time::Instant::now() >= until {
            return Err(format!(
                "Homebox started no deployment of {commit} within {} seconds of the push; its \
                 webhook for {} may not be working, or auto-deploy may be off in Homebox",
                timing.appear_within.as_secs(),
                project.repo_full_name
            ));
        }
        backoff.wait().await;
    }
}

/// Polls the deployment until it is no longer on its way. Running is success; anything else
/// says why.
async fn finished(
    hb: &Homebox<'_>,
    project: &Project,
    id: &str,
    timing: &Timing,
) -> Result<Deployment, String> {
    let until = tokio::time::Instant::now() + timing.finish_within;
    let mut backoff = Backoff::new(timing);
    loop {
        let d = hb.deployment(&project.id, id).await.map_err(says)?;
        if !d.in_progress() {
            return match d.status.as_str() {
                "running" => Ok(d),
                "staged" => Err(
                    "the deploy is staged: the environment is paused in Homebox; resume it there"
                        .into(),
                ),
                status => Err(failure(&d, status)),
            };
        }
        if tokio::time::Instant::now() >= until {
            return Err(format!(
                "the deployment was still {} after {} minutes",
                d.status,
                timing.finish_within.as_secs() / 60
            ));
        }
        backoff.wait().await;
    }
}

fn failure(d: &Deployment, status: &str) -> String {
    let mut why = format!("the deploy {status}");
    if let Some(error) = d.error.as_deref().filter(|e| !e.trim().is_empty()) {
        why.push_str(&format!(": {}", short(error)));
    }
    if let Some(log) = d.log_tail.as_deref().filter(|l| !l.trim().is_empty()) {
        why.push_str(&format!(". The end of its log: {}", tail(log, 400)));
    }
    why
}

/// The last `n` characters of a log, on one line.
fn tail(text: &str, n: usize) -> String {
    let line = short_all(text);
    let count = line.chars().count();
    line.chars().skip(count.saturating_sub(n)).collect()
}

fn short_all(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Waits for the public instances of deployment `id` to answer, and returns the address. Homebox
/// probes only an instance with an address, so one without (a database, a worker) is not waited
/// on. A running deployment whose container does not answer is a failure: it may be crashing over
/// and over.
async fn answering(
    hb: &Homebox<'_>,
    project: &Project,
    env: &str,
    id: &str,
    timing: &Timing,
) -> Result<Option<String>, String> {
    let mut backoff = Backoff::new(timing);
    let mut seen = Vec::new();
    for poll in 0..timing.settle_polls.max(1) {
        if poll > 0 {
            backoff.wait().await;
        }
        let now = hb.project(&project.id).await.map_err(says)?;
        let e = environment(&now, env)?;
        if !e.deployment.as_ref().is_some_and(|d| d.id == id) {
            continue;
        }
        let public: Vec<_> = e.instances.iter().filter(|i| i.url.is_some()).collect();
        if public.is_empty() {
            // Nothing Homebox would probe: live as far as anyone can tell.
            return Ok(None);
        }
        if public.iter().all(|i| i.status == "running") {
            return Ok(public.iter().find_map(|i| i.url.clone()));
        }
        seen = public
            .iter()
            .map(|i| {
                format!(
                    "{} is {}",
                    i.service_name.as_deref().unwrap_or("a service"),
                    i.status
                )
            })
            .collect();
        if public.iter().any(|i| i.status == "unreachable") {
            break;
        }
    }
    if seen.is_empty() {
        return Err("the deployment is running but Homebox no longer shows it as current".into());
    }
    Err(format!(
        "the deployment is running but its container does not answer ({}): it may be crashing \
         over and over",
        seen.join(", ")
    ))
}
