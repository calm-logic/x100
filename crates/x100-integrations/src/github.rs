//! GitHub, where a project's repository goes so Homebox can deploy it.
//!
//! Two ways to connect. "Use local" runs the builder's own `gh` command line, logged in on this
//! machine, for the few things x100 needs: who is logged in, creating a private repository, and
//! pushing with `gh` as git's credential helper. The daemon never reads or holds `gh`'s token.
//! The other is GitHub's device flow for an OAuth app registered for x100, whose token goes to the
//! vault; it needs the app's client id, which does not exist until the owner registers one.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use x100_core::id::CredentialRef;
use x100_vault::{CallMethod, DaemonCall, Proxy, Scheme, SecretValue};

use crate::api::{self, Endpoint};

/// The hosts a device-flow token may be sent to.
pub const OAUTH_HOSTS: [&str; 2] = ["github.com", "api.github.com"];

/// The scope a device-flow token asks for: creating and pushing to private repositories.
const SCOPE: &str = "repo";

/// How long one command may take. A push of a small project takes seconds.
const WITHIN: Duration = Duration::from_secs(120);

#[async_trait]
pub trait GitHubCli: Send + Sync {
    /// Whether `gh` is installed and logged in: `gh auth status`'s exit code alone.
    async fn logged_in(&self) -> bool;
    /// The account `gh` is logged in as.
    async fn account(&self) -> Result<String, String>;
    /// Creates `owner/name` as a private repository.
    async fn create_private(&self, full_name: &str) -> Result<(), String>;
    /// Pushes the local `branch` of the repository at `dir` to the same branch of `full_name`,
    /// never forcing.
    async fn push(&self, dir: &Path, full_name: &str, branch: &str) -> Result<(), String>;
}

/// The real one: the `gh` and `git` command lines.
pub struct Gh {
    pub program: PathBuf,
    /// Where repositories are pushed: `https://github.com` unless a test names another.
    pub git_base: String,
}

impl Gh {
    pub fn push_url(&self, full_name: &str) -> String {
        format!("{}/{full_name}.git", self.git_base.trim_end_matches('/'))
    }

    /// The commands `push` runs, for a reader and a test: git with `gh` as its only credential
    /// helper.
    pub fn push_args(&self, dir: &Path, full_name: &str, branch: &str) -> Vec<String> {
        let helper = format!(
            "credential.helper=!'{}' auth git-credential",
            self.program.display()
        );
        vec![
            "-C".into(),
            dir.display().to_string(),
            "-c".into(),
            // An empty helper first clears any the builder configured, so only gh's is asked.
            "credential.helper=".into(),
            "-c".into(),
            helper,
            "push".into(),
            "--porcelain".into(),
            self.push_url(full_name),
            format!("refs/heads/{branch}:refs/heads/{branch}"),
        ]
    }
}

#[async_trait]
impl GitHubCli for Gh {
    async fn logged_in(&self) -> bool {
        let status = tokio::process::Command::new(&self.program)
            .args(["auth", "status"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status();
        // Asked whenever Settings shows Integrations, so it may not take long.
        let within = Duration::from_secs(10);
        matches!(tokio::time::timeout(within, status).await, Ok(Ok(s)) if s.success())
    }

    async fn account(&self) -> Result<String, String> {
        let out = run(&self.program, &["api", "user", "--jq", ".login"]).await?;
        let login = out.trim();
        if login.is_empty() {
            return Err("gh did not say which account it is logged in as".into());
        }
        Ok(login.to_owned())
    }

    async fn create_private(&self, full_name: &str) -> Result<(), String> {
        run(
            &self.program,
            &["repo", "create", full_name, "--private", "--disable-wiki"],
        )
        .await
        .map(drop)
    }

    async fn push(&self, dir: &Path, full_name: &str, branch: &str) -> Result<(), String> {
        let args = self.push_args(dir, full_name, branch);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run(Path::new("git"), &args).await.map(drop)
    }
}

/// Runs a command with prompts off, and returns its output or why it failed.
async fn run(program: &Path, args: &[&str]) -> Result<String, String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    match tokio::time::timeout(WITHIN, out).await {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into()),
        Ok(Ok(out)) => Err(format!(
            "{name} {what} failed: {}",
            api::short(&String::from_utf8_lossy(&out.stderr))
        )),
        Ok(Err(e)) => Err(format!("{name} could not be run: {e}")),
        Err(_) => Err(format!("{name} {what} did not finish in time")),
    }
}

/// What the builder is shown to approve x100 on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    /// Not a credential, but it is what the polling presents, so it stays in the daemon.
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default = "five")]
    pub interval: u64,
    #[serde(default)]
    pub expires_in: u64,
}

fn five() -> u64 {
    5
}

fn github() -> Endpoint {
    Endpoint {
        scheme: Scheme::Https,
        host: "github.com".into(),
        port: None,
        credential: None,
    }
}

/// Starts GitHub's device flow for the OAuth app `client_id`.
pub async fn device_start(proxy: &Proxy, client_id: &str) -> Result<DeviceCode, String> {
    let answer = api::post_form(
        proxy,
        &github(),
        "/login/device/code",
        &[("client_id", client_id), ("scope", SCOPE)],
    )
    .await
    .map_err(|e| e.why)?;
    serde_json::from_value(answer)
        .map_err(|_| "GitHub's answer to the device flow is not one it documents".to_owned())
}

/// One poll of the device flow.
pub enum Polled {
    /// The builder has not approved yet; ask again after the interval, longer if `slower`.
    Waiting {
        slower: bool,
    },
    Token(SecretValue),
    /// Denied, expired, or refused: the flow is over.
    Over(String),
}

/// Asks whether the builder approved. The token arrives in the answer's body, which is read into
/// a value that wipes itself and then wiped.
pub async fn device_poll(proxy: &Proxy, client_id: &str, device_code: &str) -> Polled {
    let at = github();
    let body = format!(
        "client_id={client_id}&device_code={device_code}&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
    );
    let call = DaemonCall {
        method: CallMethod::Post,
        scheme: at.scheme,
        host: at.host.clone(),
        port: at.port,
        path: "/login/oauth/access_token".into(),
        credential: None,
        headers: vec![
            ("accept".into(), "application/json".into()),
            (
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
        ],
        body: body.into_bytes(),
        timeout: Duration::from_secs(20),
    };
    let mut body = match proxy.call(call).await.answer {
        Ok((200, body)) => body,
        Ok((status, _)) => return Polled::Over(format!("GitHub answered {status}")),
        Err(why) => return Polled::Over(why),
    };
    #[derive(Deserialize)]
    struct Answer {
        access_token: Option<String>,
        error: Option<String>,
    }
    let read: Option<Answer> = serde_json::from_slice(&body).ok();
    body.fill(0);
    std::hint::black_box(&body);
    match read {
        Some(Answer {
            access_token: Some(token),
            ..
        }) => Polled::Token(SecretValue::new(token.into_bytes())),
        Some(Answer {
            error: Some(error), ..
        }) => match error.as_str() {
            "authorization_pending" => Polled::Waiting { slower: false },
            "slow_down" => Polled::Waiting { slower: true },
            "expired_token" => Polled::Over("the code expired before it was approved".into()),
            "access_denied" => Polled::Over("the builder denied x100 on GitHub".into()),
            other => Polled::Over(format!("GitHub refused: {other}")),
        },
        _ => Polled::Over("GitHub's answer is not one it documents".into()),
    }
}

/// The account a device-flow token belongs to.
pub async fn oauth_account(proxy: &Proxy, credential: CredentialRef) -> Result<String, String> {
    let at = Endpoint {
        scheme: Scheme::Https,
        host: "api.github.com".into(),
        port: None,
        credential: Some(credential),
    };
    let user: Value = api::get(proxy, &at, "/user").await.map_err(|e| e.why)?;
    user.get("login")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "GitHub did not say whose token it is".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_asks_only_gh_for_credentials_and_never_forces() {
        let gh = Gh {
            program: "/usr/bin/gh".into(),
            git_base: "https://github.com/".into(),
        };
        let args = gh.push_args(Path::new("/p"), "me/todo", "main");
        assert_eq!(
            args,
            [
                "-C",
                "/p",
                "-c",
                "credential.helper=",
                "-c",
                "credential.helper=!'/usr/bin/gh' auth git-credential",
                "push",
                "--porcelain",
                "https://github.com/me/todo.git",
                "refs/heads/main:refs/heads/main",
            ]
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--force") || a.starts_with('+'))
        );
    }
}
