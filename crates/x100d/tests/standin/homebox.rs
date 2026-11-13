//! A stand-in for Homebox's admin API, the part x100 calls, typed after Homebox's route handlers,
//! and for GitHub's device flow. It serves plain HTTP on loopback, as a local Homebox does, and
//! the same answers over any stream a test hands it, such as TLS for a Homebox elsewhere.
//!
//! Keys are `hbk_` keys with a role, sent as `Authorization: Bearer`; a read key may only GET. A
//! project is one repository among the bare repositories under `remotes`, as
//! `<owner>/<name>.git`. A managed project with `auto_deploy` deploys what is pushed to its main
//! branch as a webhook would, noticed whenever the stand-in is next asked about it. A deployment
//! goes from queued through building to running, or to failed for a project set to fail, one
//! step each time it is read; a project set to crash runs with an instance that does not answer.
//! Self-contained, so the app's tests include it too.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

/// The domain a deploy is public under.
pub const DOMAIN: &str = "box.x100.test";
/// The GitHub account Homebox's integration covers, and the one `gh` is logged in as.
pub const ACCOUNT: &str = "builder";
/// What GitHub's device flow gives, invented for the tests.
pub const DEVICE_TOKEN: &str = "gho_standin_token_5d1c9a7e3b2f";

#[derive(Debug, Clone)]
pub struct Spec {
    pub name: &'static str,
    pub managed: bool,
    pub auto_deploy: bool,
    pub fails: bool,
    pub crashes: bool,
    /// Its deployments stay queued.
    pub stalls: bool,
}

impl Spec {
    pub fn managed(name: &'static str, auto_deploy: bool) -> Spec {
        Spec {
            name,
            managed: true,
            auto_deploy,
            fails: false,
            crashes: false,
            stalls: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Project {
    id: u64,
    name: String,
    repo: String,
    managed: bool,
    auto_deploy: bool,
    fails: bool,
    crashes: bool,
    stalls: bool,
    /// Production first, then dev, once managed.
    envs: Vec<(u64, String)>,
    /// The head of main the webhook last saw.
    seen: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    pub id: u64,
    pub project: u64,
    pub env: u64,
    pub commit: Option<String>,
    pub trigger: String,
    reads: u32,
}

/// One request the stand-in was sent.
#[derive(Debug, Clone)]
pub struct Received {
    pub method: String,
    pub host: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Default)]
struct State {
    keys: HashMap<String, &'static str>,
    projects: Vec<Project>,
    deployments: Vec<Deployment>,
    /// Deploys asked for, by project id.
    deploy_calls: HashMap<u64, usize>,
    /// What projects found by a sync start with.
    new_auto_deploy: bool,
    received: Vec<Received>,
    next: u64,
    /// Device-flow polls answered so far: the first is pending.
    polls: u32,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

#[derive(Clone)]
pub struct HomeboxStandIn {
    pub port: u16,
    remotes: PathBuf,
    state: Arc<Mutex<State>>,
}

impl HomeboxStandIn {
    /// Listens on a loopback port. Repositories are the bare ones under `remotes`.
    pub async fn start(remotes: &Path) -> HomeboxStandIn {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = HomeboxStandIn {
            port,
            remotes: remotes.to_owned(),
            state: Arc::new(Mutex::new(State {
                new_auto_deploy: true,
                ..State::default()
            })),
        };
        let served = me.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let s = served.clone();
                tokio::spawn(async move { s.serve(tcp).await });
            }
        });
        me
    }

    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn key(&self, key: &str, role: &'static str) {
        self.state.lock().unwrap().keys.insert(key.into(), role);
    }

    /// A project of a repository `builder/<name>`, which is made bare under `remotes` if it is
    /// not there.
    pub fn project(&self, spec: Spec) -> u64 {
        let repo = format!("{ACCOUNT}/{}", spec.name);
        let bare = self.bare(&repo);
        if !bare.exists() {
            git(&[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &bare.to_string_lossy(),
            ]);
        }
        let mut s = self.state.lock().unwrap();
        let id = s.id();
        let mut p = Project {
            id,
            name: spec.name.into(),
            repo,
            managed: false,
            auto_deploy: spec.auto_deploy,
            fails: spec.fails,
            crashes: spec.crashes,
            stalls: spec.stalls,
            envs: Vec::new(),
            seen: None,
        };
        if spec.managed {
            manage(&mut s, &mut p, &self.remotes);
        }
        s.projects.push(p);
        id
    }

    /// Whether a project a sync finds deploys on push.
    pub fn new_projects_auto_deploy(&self, on: bool) {
        self.state.lock().unwrap().new_auto_deploy = on;
    }

    pub fn received(&self) -> Vec<Received> {
        self.state.lock().unwrap().received.clone()
    }

    pub fn deploy_calls(&self, name: &str) -> usize {
        let s = self.state.lock().unwrap();
        s.projects
            .iter()
            .find(|p| p.name == name)
            .and_then(|p| s.deploy_calls.get(&p.id).copied())
            .unwrap_or(0)
    }

    /// Every deployment of the project's production environment of `commit`.
    pub fn deployments_of(&self, name: &str, commit: &str) -> Vec<Deployment> {
        let s = self.state.lock().unwrap();
        let Some(p) = s.projects.iter().find(|p| p.name == name) else {
            return Vec::new();
        };
        let production = p.envs.first().map(|e| e.0);
        s.deployments
            .iter()
            .filter(|d| {
                d.project == p.id
                    && Some(d.env) == production
                    && d.commit.as_deref() == Some(commit)
            })
            .cloned()
            .collect()
    }

    pub fn managed(&self, name: &str) -> bool {
        let s = self.state.lock().unwrap();
        s.projects.iter().any(|p| p.name == name && p.managed)
    }

    pub fn bare(&self, repo: &str) -> PathBuf {
        self.remotes.join(format!("{repo}.git"))
    }

    /// What the public address `host` serves, as a static site would: `index.html` of the commit
    /// the project's production deployment runs, once it runs.
    pub fn site(&self, host: &str) -> Option<String> {
        let name = host
            .split(':')
            .next()?
            .strip_suffix(&format!(".{DOMAIN}"))?;
        let s = self.state.lock().unwrap();
        let p = s.projects.iter().find(|p| p.name == name)?;
        let production = p.envs.first()?.0;
        let running = s
            .deployments
            .iter()
            .rev()
            .find(|d| d.project == p.id && d.env == production && status(&s, d) == "running")?;
        let spec = format!("{}:index.html", running.commit.as_deref()?);
        let out = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(self.bare(&p.repo))
            .args(["show", &spec])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Answers one connection's requests, plain or over TLS.
    pub async fn serve<S: AsyncRead + AsyncWrite + Unpin>(&self, mut s: S) {
        let Some(req) = read(&mut s).await else {
            return;
        };
        let (status, body) = self.answer(&req);
        let body = body.to_string();
        let head = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = s.write_all(head.as_bytes()).await;
        let _ = s.write_all(body.as_bytes()).await;
        let _ = s.shutdown().await;
    }

    fn answer(&self, req: &Received) -> (u16, Value) {
        let mut s = self.state.lock().unwrap();
        s.received.push(req.clone());
        let host = req.host.split(':').next().unwrap_or_default();
        if host == "github.com" || host == "api.github.com" {
            return github(&mut s, host, req);
        }
        let path = req.path.split('?').next().unwrap_or_default();
        match (req.method.as_str(), path) {
            ("GET", "/api/healthz") => return (200, json!({"ok": true})),
            ("GET", "/api/openapi.json") => {
                return (
                    200,
                    json!({"openapi": "3.1.0", "info": {"title": "Homebox Admin API"}}),
                );
            }
            _ => {}
        }
        let key = req
            .authorization
            .as_deref()
            .and_then(|a| a.strip_prefix("Bearer "))
            .unwrap_or_default();
        let Some(role) = s.keys.get(key).copied() else {
            return (401, json!({"detail": "Not authenticated"}));
        };
        if req.method != "GET" && role == "read" {
            return (403, json!({"detail": "This API key is read-only."}));
        }
        let parts: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
        let remotes = self.remotes.clone();
        match (req.method.as_str(), parts.as_slice()) {
            ("GET", ["auth", "me"]) => (200, json!({"username": "key:x100", "role": role})),
            ("GET", ["domains"]) => (200, json!([{"id": 1, "name": DOMAIN, "is_primary": true}])),
            ("GET", ["integrations"]) => (
                200,
                json!([{"id": 1, "provider": "github", "account_login": ACCOUNT, "orgs": []}]),
            ),
            ("POST", ["integrations", "1", "sync"]) => {
                let found = sync(&mut s, &remotes);
                (200, json!({"ok": true, "synced": found}))
            }
            ("GET", ["projects"]) => {
                let ids: Vec<u64> = s.projects.iter().map(|p| p.id).collect();
                let all: Vec<Value> = ids
                    .into_iter()
                    .map(|id| project_json(&mut s, id, &remotes))
                    .collect();
                (200, Value::Array(all))
            }
            ("GET", ["projects", id]) => match find(&s, id) {
                Some(id) => (200, project_json(&mut s, id, &remotes)),
                None => (404, json!({"detail": "Project not found"})),
            },
            ("POST", ["projects", id, "add"]) => match find(&s, id) {
                Some(id) => add(&mut s, id, &remotes),
                None => (404, json!({"detail": "Project not found"})),
            },
            ("POST", ["projects", id, "environments", env, "deploy"]) => {
                let Some(id) = find(&s, id) else {
                    return (404, json!({"detail": "Project not found"}));
                };
                let env: u64 = env.parse().unwrap_or(0);
                *s.deploy_calls.entry(id).or_default() += 1;
                let head = head(&remotes, &repo_of(&s, id));
                let dep = queue(&mut s, id, env, head, "manual");
                (
                    200,
                    json!({"id": dep, "status": "queued", "environment": "production", "staged": false}),
                )
            }
            ("GET", ["projects", id, "environments", env, "deployments"]) => {
                let Some(id) = find(&s, id) else {
                    return (404, json!({"detail": "Project not found"}));
                };
                webhook(&mut s, id, &remotes);
                let env: u64 = env.parse().unwrap_or(0);
                let list: Vec<Value> = s
                    .deployments
                    .iter()
                    .rev()
                    .filter(|d| d.project == id && d.env == env)
                    .map(|d| {
                        json!({"id": d.id, "status": status(&s, d), "commit_sha": d.commit,
                               "trigger": d.trigger, "error": null})
                    })
                    .collect();
                (200, Value::Array(list))
            }
            ("GET", ["projects", id, "deployments", dep]) => {
                let Some(id) = find(&s, id) else {
                    return (404, json!({"detail": "Project not found"}));
                };
                let dep: u64 = dep.parse().unwrap_or(0);
                let Some(i) = s
                    .deployments
                    .iter()
                    .position(|d| d.id == dep && d.project == id)
                else {
                    return (404, json!({"detail": "Deployment not found"}));
                };
                s.deployments[i].reads += 1;
                let d = s.deployments[i].clone();
                let st = status(&s, &d);
                let failed = st == "failed";
                (
                    200,
                    json!({
                        "id": d.id, "status": st, "commit_sha": d.commit, "trigger": d.trigger,
                        "error": failed.then_some("Build failed: npm ERR! Missing script: \"build\""),
                        "log_tail": failed.then_some("> npm run build\nnpm ERR! Missing script: \"build\"\nexit status 1"),
                        "log_steps": [],
                    }),
                )
            }
            _ => (404, json!({"detail": "Not Found"})),
        }
    }
}

fn find(s: &State, id: &str) -> Option<u64> {
    let id: u64 = id.parse().ok()?;
    s.projects.iter().any(|p| p.id == id).then_some(id)
}

fn repo_of(s: &State, id: u64) -> String {
    s.projects.iter().find(|p| p.id == id).unwrap().repo.clone()
}

/// A deployment's status, from how often it was read: queued, building, then running or failed.
fn status(s: &State, d: &Deployment) -> &'static str {
    let p = s.projects.iter().find(|p| p.id == d.project);
    let fails = p.is_some_and(|p| p.fails);
    match d.reads {
        _ if p.is_some_and(|p| p.stalls) => "queued",
        0 => "queued",
        1 => "building",
        _ if fails => "failed",
        _ => "running",
    }
}

fn queue(s: &mut State, project: u64, env: u64, commit: Option<String>, trigger: &str) -> u64 {
    let id = s.id();
    s.deployments.push(Deployment {
        id,
        project,
        env,
        commit,
        trigger: trigger.into(),
        reads: 0,
    });
    id
}

/// Makes the project managed with its two environments, and notes what its webhook has seen.
fn manage(s: &mut State, p: &mut Project, remotes: &Path) {
    p.managed = true;
    let (prod, dev) = (s.id(), s.id());
    p.envs = vec![(prod, "production".into()), (dev, "dev".into())];
    p.seen = head(remotes, &p.repo);
}

fn add(s: &mut State, id: u64, remotes: &Path) -> (u16, Value) {
    let i = s.projects.iter().position(|p| p.id == id).unwrap();
    let mut p = s.projects[i].clone();
    if p.managed {
        return (
            200,
            json!({"ok": true, "id": id, "name": p.name, "note": null, "deployments": []}),
        );
    }
    manage(s, &mut p, remotes);
    let head = p.seen.clone();
    let mut deployments = Vec::new();
    for (env, name) in p.envs.clone() {
        let dep = queue(s, id, env, head.clone(), "initial");
        deployments.push(json!({"id": dep, "status": "queued", "environment": name}));
    }
    let name = p.name.clone();
    s.projects[i] = p;
    (
        200,
        json!({"ok": true, "id": id, "name": name, "note": null, "webhook_note": null,
               "deployments": deployments}),
    )
}

/// A deployment of production for each new head of main of a managed project with auto-deploy.
fn webhook(s: &mut State, id: u64, remotes: &Path) {
    let i = s.projects.iter().position(|p| p.id == id).unwrap();
    let p = s.projects[i].clone();
    if !p.managed || !p.auto_deploy {
        return;
    }
    let now = head(remotes, &p.repo);
    if now.is_some() && now != p.seen {
        s.projects[i].seen = now.clone();
        queue(s, id, p.envs[0].0, now, "push");
    }
}

/// Projects for the bare repositories of the account that have none yet.
fn sync(s: &mut State, remotes: &Path) -> usize {
    let Ok(dir) = std::fs::read_dir(remotes.join(ACCOUNT)) else {
        return 0;
    };
    let mut found = 0;
    for entry in dir.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(name) = file.strip_suffix(".git") else {
            continue;
        };
        let repo = format!("{ACCOUNT}/{name}");
        if s.projects.iter().any(|p| p.repo == repo) {
            continue;
        }
        let id = s.id();
        let auto_deploy = s.new_auto_deploy;
        s.projects.push(Project {
            id,
            name: name.into(),
            repo,
            managed: false,
            auto_deploy,
            fails: false,
            crashes: false,
            stalls: false,
            envs: Vec::new(),
            seen: None,
        });
        found += 1;
    }
    found
}

fn project_json(s: &mut State, id: u64, remotes: &Path) -> Value {
    webhook(s, id, remotes);
    let p = s.projects.iter().find(|p| p.id == id).unwrap().clone();
    let envs: Vec<Value> = p
        .envs
        .iter()
        .map(|(env, name)| {
            let latest = s
                .deployments
                .iter()
                .rev()
                .find(|d| d.project == id && d.env == *env)
                .cloned();
            let deployment = latest.as_ref().map(|d| {
                json!({"id": d.id, "status": status(s, d), "commit_sha": d.commit, "error": null})
            });
            let running = latest.as_ref().is_some_and(|d| status(s, d) == "running");
            let instances = if running {
                let host = if name == "production" {
                    format!("{}.{DOMAIN}", p.name)
                } else {
                    format!("{}-{name}.{DOMAIN}", p.name)
                };
                vec![
                    json!({"service_name": "web", "url": format!("https://{host}"),
                           "status": if p.crashes { "unreachable" } else { "running" },
                           "container_name": "web-1"}),
                    // A database has no address, and Homebox never probes it.
                    json!({"service_name": "db", "url": null, "status": "unknown",
                           "container_name": "db-1"}),
                ]
            } else {
                Vec::new()
            };
            json!({"id": env, "name": name, "deployment": deployment, "instances": instances})
        })
        .collect();
    json!({
        "id": p.id, "name": p.name, "repo_full_name": p.repo, "managed": p.managed,
        "auto_deploy": p.auto_deploy, "domain": null,
        "integration": {"id": 1, "provider": "github", "account_login": ACCOUNT},
        "environments": envs,
    })
}

/// GitHub's device flow: a code, one pending poll, then a token; and whose token it is.
fn github(s: &mut State, host: &str, req: &Received) -> (u16, Value) {
    match (host, req.method.as_str(), req.path.as_str()) {
        ("github.com", "POST", "/login/device/code") => (
            200,
            json!({"device_code": "standin-device-code", "user_code": "WDJB-MJHT",
                   "verification_uri": "https://github.com/login/device", "interval": 1,
                   "expires_in": 900}),
        ),
        ("github.com", "POST", "/login/oauth/access_token") => {
            s.polls += 1;
            if s.polls == 1 {
                (200, json!({"error": "authorization_pending"}))
            } else {
                (
                    200,
                    json!({"access_token": DEVICE_TOKEN, "token_type": "bearer", "scope": "repo"}),
                )
            }
        }
        ("api.github.com", "GET", "/user") => {
            if req.authorization.as_deref() == Some(&format!("Bearer {DEVICE_TOKEN}")) {
                (200, json!({"login": ACCOUNT}))
            } else {
                (401, json!({"message": "Bad credentials"}))
            }
        }
        _ => (404, json!({"message": "Not Found"})),
    }
}

/// A stand-in `docker` program: `docker port` names the stand-in's `port` when the container is
/// `running`; `docker exec` logs its first six arguments, the command without its script, to
/// `docker.log` in `bin`, and prints `key` when the script mints an edit key, as "Use local"'s
/// does.
pub fn docker_program(running: bool, port: u16, bin: &Path, key: &str) -> String {
    let log = bin.join("docker.log");
    let port_line = if running {
        format!("echo 127.0.0.1:{port}; exit 0")
    } else {
        "echo 'Error: No such container: homebox-admin' >&2; exit 1".into()
    };
    format!(
        "case \"$1\" in\n\
         port) [ \"$2 $3\" = 'homebox-admin 8000/tcp' ] || exit 1; {port_line};;\n\
         exec) echo \"$1 $2 $3 $4 $5 $6\" >> '{log}'\n\
         case \"$7\" in *'role=\"edit\"'*) echo '{key}';; *) echo 'the stand-in ran a script';; esac\n\
         exit 0;;\n\
         esac\nexit 1\n",
        log = log.display()
    )
}

/// A stand-in `gh` program, logged in as [`ACCOUNT`] or not at all. It logs each command to
/// `gh.log` in `bin` and makes a created repository a bare one under `remotes`.
pub fn gh_program(logged_in: bool, remotes: &Path, bin: &Path) -> String {
    if !logged_in {
        return "echo 'You are not logged into any GitHub hosts.' >&2\nexit 1\n".into();
    }
    format!(
        "echo \"$*\" >> '{log}'\n\
         case \"$1 $2\" in\n\
         'auth status') exit 0;;\n\
         'api user') echo {ACCOUNT}; exit 0;;\n\
         'repo create') mkdir -p '{remotes}/'\"$(dirname \"$3\")\" && git init -q --bare -b main '{remotes}/'\"$3\".git; exit $?;;\n\
         'auth git-credential') exit 0;;\n\
         esac\nexit 1\n",
        log = bin.join("gh.log").display(),
        remotes = remotes.display()
    )
}

/// The commit main points at in a bare repository, if it has one.
pub fn head(remotes: &Path, repo: &str) -> Option<String> {
    let bare = remotes.join(format!("{repo}.git"));
    let out = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&bare)
        .args(["rev-parse", "--verify", "-q", "refs/heads/main"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn git(args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

async fn read<S: AsyncRead + Unpin>(s: &mut S) -> Option<Received> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        })
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end..].to_vec();
    while body.len() < length {
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Received {
        method,
        host: headers.get("host").cloned().unwrap_or_default(),
        path,
        authorization: headers.get("authorization").cloned(),
        body,
    })
}
