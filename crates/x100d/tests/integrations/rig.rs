//! The rig for `integrations.rs`: the daemon with its stand-ins, and what the tests ask of it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use x100_client::{Client, ClientError};
use x100_core::action::{ActionState, AskMeAbout, Decision, Request};
use x100_core::fold::{ActionRecord, ProjectState};
use x100_core::id::{ActionId, AgentId, ProjectId};
use x100_core::project::SettingChange;
use x100_core::tool::ToolCall;
use x100_core::transcript::Item;
use x100_proto::{
    ChangeProjectSetting, DecideAction, ErrorCode, GetDaemon, LogId, LogPage, Page, ReadLog,
    ReadTranscript,
};

use crate::common::{DaemonProc, Env, WAIT, create_project};
use crate::standin;
use crate::standin::homebox::{ACCOUNT, HomeboxStandIn};

/// Invented for these tests; none opens anything anywhere.
pub const LOCAL_KEY: &str = "hbk_minted_by_the_standin_8c2f4a9d1e7b";
pub const EDIT_KEY: &str = "hbk_pasted_edit_key_3e9a7c1d5b20f4";
pub const READ_KEY: &str = "hbk_pasted_read_key_6f1b2d8e4a93c7";
pub const REMOTE_HOST: &str = "homebox.x100.test";

pub const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "LINK" }
repeat = true
steps = [{ call = { link_project = { homebox_project = "{{input:Homebox}}" } } }]

[[turn]]
when = { input_contains = "DEPLOY" }
repeat = true
steps = [{ call = { deploy = {} } }]

[[turn]]
repeat = true
steps = [{ text = "noted: {{input}}" }]
"##;

#[derive(Clone, Copy)]
pub struct Has {
    /// A `homebox-admin` container is running here.
    pub local: bool,
    /// `gh` is installed and logged in.
    pub gh: bool,
    /// The daemon is given an OAuth app's client id.
    pub client_id: bool,
}

pub struct Rig {
    pub env: Env,
    /// The stand-in programs, outside the test's directory, since `docker` prints the key.
    pub _bin: tempfile::TempDir,
    pub bin: PathBuf,
    pub remotes: PathBuf,
    pub hb: HomeboxStandIn,
    pub args: Vec<std::ffi::OsString>,
    pub vars: Vec<(&'static str, String)>,
    pub daemon: DaemonProc,
    pub client: Client,
}

impl Rig {
    pub async fn start(has: Has) -> Rig {
        let env = Env::new();
        std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
        let bin_dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let bin = bin_dir.path().to_owned();
        let remotes = env.dir.path().join("github");
        std::fs::create_dir_all(remotes.join(ACCOUNT)).unwrap();
        let hb = HomeboxStandIn::start(&remotes).await;
        for (key, role) in [(LOCAL_KEY, "edit"), (EDIT_KEY, "edit"), (READ_KEY, "read")] {
            hb.key(key, role);
        }
        let ca = bin.join("ca.pem");
        let acceptor = standin::tls(
            &ca,
            [REMOTE_HOST, "github.com", "api.github.com"]
                .map(String::from)
                .to_vec(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tls_port = listener.local_addr().unwrap().port();
        let served = hb.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (acceptor, hb) = (acceptor.clone(), served.clone());
                tokio::spawn(async move {
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        hb.serve(tls).await;
                    }
                });
            }
        });
        script(&bin, "docker", &docker(has.local, hb.port, &bin));
        script(&bin, "gh", &gh(has.gh, &remotes, &bin));
        let resolve = [REMOTE_HOST, "github.com", "api.github.com"]
            .map(|h| format!("{h}=127.0.0.1:{tls_port}"))
            .join(",");
        let mut vars = vec![
            (x100d::PROXY_RESOLVE_ENV, resolve),
            (x100d::PROXY_UPSTREAM_CA_ENV, ca.display().to_string()),
            (x100d::DOCKER_ENV, bin.join("docker").display().to_string()),
            (x100d::GH_ENV, bin.join("gh").display().to_string()),
            (x100d::GIT_BASE_ENV, format!("file://{}", remotes.display())),
        ];
        if has.client_id {
            vars.push((x100d::GITHUB_CLIENT_ID_ENV, "Iv1.standin".into()));
        }
        let mut args = env.daemon_args(Path::new(env!("CARGO_BIN_EXE_x100d-fake-harness")));
        args.extend(["--fake-clock".into(), "1794000000000".into()]);
        let daemon = DaemonProc::spawn_adding(args.clone(), &vars);
        let client = env.client().await;
        Rig {
            env,
            _bin: bin_dir,
            bin,
            remotes,
            hb,
            args,
            vars,
            daemon,
            client,
        }
    }

    pub async fn restart(&mut self) {
        self.daemon.terminate();
        self.daemon = DaemonProc::spawn_adding(self.args.clone(), &self.vars);
        self.client = self.env.client().await;
    }

    /// What the stand-in programs were asked to do, a line each.
    pub fn ran(&self, program: &str) -> Vec<String> {
        std::fs::read_to_string(self.bin.join(format!("{program}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub async fn connect_local(&self) {
        self.client
            .connect_local_homebox()
            .await
            .expect("connected");
    }
}

pub fn script(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `docker port` names the stand-in's port when the container is "running"; `docker exec` logs
/// its first six arguments, the command without the script, and prints a key.
pub fn docker(running: bool, port: u16, bin: &Path) -> String {
    let log = bin.join("docker.log");
    let port_line = if running {
        format!("echo 127.0.0.1:{port}; exit 0")
    } else {
        "echo 'Error: No such container: homebox-admin' >&2; exit 1".into()
    };
    format!(
        "case \"$1\" in\n\
         port) [ \"$2 $3\" = 'homebox-admin 8000/tcp' ] || exit 1; {port_line};;\n\
         exec) echo \"$1 $2 $3 $4 $5 $6\" >> '{log}'; echo '{LOCAL_KEY}'; exit 0;;\n\
         esac\nexit 1\n",
        log = log.display()
    )
}

/// `gh`, logged in as the stand-in's account, or not logged in at all.
pub fn gh(logged_in: bool, remotes: &Path, bin: &Path) -> String {
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

pub struct Project {
    pub id: ProjectId,
    pub ceo: AgentId,
    pub dir: PathBuf,
}

pub async fn project(r: &Rig, name: &str) -> Project {
    let id = create_project(&r.client, &r.env, name).await;
    r.client
        .request(ChangeProjectSetting {
            project: id,
            change: SettingChange::AskMeAbout(AskMeAbout::HighRiskOnly),
        })
        .await
        .unwrap();
    let ceo = r.client.project(id).unwrap().project().unwrap().ceo;
    Project {
        id,
        ceo,
        dir: PathBuf::from(r.env.project_dir(name)),
    }
}

/// Commits a page on the project's main branch, as a merged task would, and returns the commit.
pub fn commit(p: &Project, text: &str) -> String {
    std::fs::write(p.dir.join("index.html"), text).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&p.dir)
            .args(["-c", "user.name=test", "-c", "user.email=test@x100.test"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["add", "index.html"]);
    git(&["commit", "-q", "-m", "a page"]);
    git(&["rev-parse", "HEAD"])
}

pub fn state(r: &Rig, p: &Project) -> ProjectState {
    r.client.project(p.id).unwrap()
}

/// Waits for a deploy action the CEO asked for, other than those `before`, to reach `want`, and
/// returns it. Ids made at one moment of the fake clock do not sort by when they were made.
pub async fn deploy_action(
    r: &Rig,
    p: &Project,
    want: ActionState,
    before: &[ActionId],
) -> ActionRecord {
    r.client
        .wait_for(WAIT, |s| {
            s.projects
                .get(&p.id)?
                .actions()
                .find(|a| {
                    matches!(
                        &a.action.request,
                        Request::Tool {
                            call: ToolCall::Deploy { .. },
                            ..
                        }
                    ) && !before.contains(&a.action.id)
                        && a.state == want
                })
                .cloned()
        })
        .await
        .unwrap_or_else(|_| panic!("the deploy is {want:?}"))
}

pub async fn approve(r: &Rig, p: &Project, action: ActionId) {
    r.client
        .request(DecideAction {
            project: p.id,
            action,
            decision: Decision::Approved,
        })
        .await
        .expect("approved");
}

/// The CEO's transcript, once some text or tool result in it contains `needle`.
pub async fn ceo_saw(r: &Rig, p: &Project, needle: &str) -> String {
    let until = Instant::now() + WAIT;
    loop {
        let entries = r
            .client
            .request(ReadTranscript {
                project: p.id,
                agent: p.ceo,
                after: None,
                limit: 10_000,
            })
            .await
            .unwrap()
            .entries;
        let hit = entries.into_iter().find_map(|e| match e.entry.item {
            Item::Text { text } if text.contains(needle) => Some(text),
            Item::ToolResult { output, .. } if output.contains(needle) => Some(output),
            _ => None,
        });
        if let Some(hit) = hit {
            return hit;
        }
        assert!(Instant::now() < until, "the CEO never saw {needle:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn say(r: &Rig, p: &Project, body: &str) {
    r.client.send_chat(p.id, body, vec![], None).await.unwrap();
}

pub fn code(e: ClientError) -> (ErrorCode, String) {
    match e {
        ClientError::Rpc(e) => (e.code, e.message),
        other => panic!("not an answer from the daemon: {other:?}"),
    }
}

/// The values, raw and in the encodings a careless program might use, are in no file under the
/// test's directory (the daemon's data, the projects, the remotes), no event of either log, the
/// state, and no transcript.
pub async fn no_key_anywhere(r: &Rig, values: &[&str]) {
    let forms: Vec<String> = values
        .iter()
        .flat_map(|v| {
            let b64 = |e: &base64::engine::GeneralPurpose| e.encode(v.as_bytes());
            [
                (*v).to_owned(),
                v.bytes().map(|b| format!("{b:02x}")).collect(),
                b64(&base64::engine::general_purpose::STANDARD),
                b64(&base64::engine::general_purpose::URL_SAFE),
            ]
        })
        .collect();
    let check = |what: &str, haystack: &[u8]| {
        for form in &forms {
            assert!(
                !haystack.windows(form.len()).any(|w| w == form.as_bytes()),
                "a key is in {what}"
            );
        }
    };
    for file in files(r.env.dir.path()) {
        if let Ok(bytes) = std::fs::read(&file) {
            check(&file.display().to_string(), &bytes);
        }
    }
    let daemon = r.client.request(GetDaemon {}).await.unwrap();
    check("the daemon's state", &serde_json::to_vec(&daemon).unwrap());
    check(
        "the daemon's log",
        &serde_json::to_vec(&events(&r.client, LogId::Daemon).await).unwrap(),
    );
    let listed = r.client.list_integrations().await.unwrap();
    check("integrations.list", &serde_json::to_vec(&listed).unwrap());
    for (id, _) in daemon.projects() {
        check(
            "a project's log",
            &serde_json::to_vec(&events(&r.client, LogId::Project(id)).await).unwrap(),
        );
        let state = r.client.project(id).unwrap();
        let ceo = state.project().unwrap().ceo;
        let transcript = r
            .client
            .request(ReadTranscript {
                project: id,
                agent: ceo,
                after: None,
                limit: 10_000,
            })
            .await
            .unwrap();
        check("a transcript", &serde_json::to_vec(&transcript).unwrap());
    }
}

pub async fn events(client: &Client, log: LogId) -> Vec<serde_json::Value> {
    let page = client
        .request(ReadLog {
            log,
            page: Page::OldestFirst {
                after: None,
                limit: 10_000,
            },
        })
        .await
        .unwrap();
    match page {
        LogPage::Project { events } => events
            .into_iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect(),
        LogPage::Daemon { events } => events
            .into_iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect(),
    }
}

pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                stack.push(e.path());
            } else if kind.is_file() {
                out.push(e.path());
            }
        }
    }
    out
}
