//! Where drivers run. Each agent has one sandbox, made when its first driver starts and kept while
//! its drivers come and go, so a new driver finds the files and the processes the last one left.
//! A project's CEO before the project is set up has one over its scratch directory, replaced by
//! one over its worktree once the project has code. A driver reaches the outside only through a
//! route of its own on the vault's proxy, mapped into the sandbox for the driver's life.
//!
//! Where no sandbox can be made, no driver starts, unless the builder turned on the daemon's
//! `run_without_sandbox` setting: then drivers run as the builder, uncontained.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use x100_core::id::{AgentId, ProjectId};
use x100_core::process::ProcessName;
use x100_core::settings::DaemonSettings;
use x100_drivers::{CommandLine, CommandWrapper, Expose};
use x100_sandbox::{Backend, Mount, ProcessId, Sandbox, SandboxSpec};
use x100_vault::{Proxy, Requester, Route};

use crate::processes::ProcessRegistry;

/// How this daemon contains its drivers, as it starts.
pub struct Containment {
    /// What makes sandboxes here, or why none can be made on this machine.
    pub backend: Result<Arc<dyn Backend>, String>,
    /// Each sandbox's own directory goes under this one, removed with the sandbox. Kept short: a
    /// Codex driver's socket is under it, and a socket's path is at most about a hundred bytes.
    pub dir: PathBuf,
    /// Makes sandbox names unique among every daemon on the machine.
    pub tag: String,
    /// The programs drivers run and what they need, shown read-only in every sandbox. The
    /// daemon's `sandbox_programs` setting adds to these.
    pub programs: Vec<PathBuf>,
    /// The egress proxy and its authority's public certificate, which a sandbox trusts. `None`
    /// gives drivers no proxy, for in-process tests that have no vault.
    pub proxy: Option<(Proxy, PathBuf)>,
}

/// Where one driver runs and how it gets out, held for the driver's life.
pub(crate) struct Placed {
    pub wrap: CommandWrapper,
    pub expose: Expose,
    /// The driver's own directory, inside the sandbox's home.
    pub state: PathBuf,
    /// The sandbox's `/tmp`, on the daemon's side.
    pub tmp: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub contained: bool,
    /// Processes of an earlier sandbox of the agent's that went with it.
    pub ended: Vec<ProcessName>,
    /// Dropping it closes the driver's way out.
    _route: Option<Route>,
}

struct AgentBox {
    project: ProjectId,
    /// Unique on the machine; the process list knows its anchor by it.
    name: String,
    sandbox: Sandbox,
    /// Each process `start_process` started, and the route it reaches the outside by.
    processes: Mutex<HashMap<ProcessName, (ProcessId, Option<Route>)>>,
}

pub(crate) struct Sandboxes {
    containment: Containment,
    /// Replaced when a rescan finds the programs again.
    programs: RwLock<Vec<PathBuf>>,
    boxes: tokio::sync::Mutex<HashMap<AgentId, Arc<AgentBox>>>,
    registry: Arc<ProcessRegistry>,
}

/// The addresses a sandbox's own programs and servers use, which must not go through the proxy.
const NO_PROXY: &[&str] = &["localhost", "127.0.0.1", "::1"];

impl Sandboxes {
    pub fn new(containment: Containment, registry: Arc<ProcessRegistry>) -> Self {
        Self {
            programs: RwLock::new(containment.programs.clone()),
            containment,
            boxes: tokio::sync::Mutex::new(HashMap::new()),
            registry,
        }
    }

    /// The vault's proxy, the daemon's only way out, when there is a vault.
    pub fn proxy(&self) -> Option<&Proxy> {
        self.containment.proxy.as_ref().map(|(proxy, _)| proxy)
    }

    pub fn set_programs(&self, programs: Vec<PathBuf>) {
        *self.programs.write().expect("programs lock") = programs;
    }

    /// Places `driver` of `agent`, which works in `dir`, in the agent's sandbox, making one if
    /// it has none over `dir`. `login_hosts` are relayed untouched by its proxy route,
    /// `provider` are the hosts of its model provider, which the gate is not asked about, and
    /// `logins` are the login directories a sandbox shows read-write.
    #[allow(clippy::too_many_arguments)]
    pub async fn place(
        &self,
        settings: &DaemonSettings,
        requester: Requester,
        dir: &Path,
        tools: Option<SocketAddr>,
        logins: Vec<PathBuf>,
        login_hosts: &[String],
        provider: &[String],
    ) -> Result<Placed, String> {
        let (agent, driver) = (requester.agent, requester.driver);
        if settings.run_without_sandbox {
            eprintln!(
                "x100: driver {driver} runs without a sandbox, as the daemon's \
                 run_without_sandbox setting allows: it can do anything you can"
            );
            let state = self.containment.dir.join("uncontained").join(short(driver));
            let (route, env) = self.route(requester, login_hosts, provider, None).await?;
            return Ok(Placed {
                wrap: CommandWrapper::identity(),
                expose: Expose::nothing(),
                state,
                tmp: None,
                env,
                contained: false,
                ended: Vec::new(),
                _route: route,
            });
        }
        let backend = self.containment.backend.as_ref().map_err(|why| {
            format!(
                "no sandbox can be made on this machine ({why}), so no driver is started; the \
                 daemon setting run_without_sandbox runs drivers uncontained, which is unsafe"
            )
        })?;
        let (agent_box, ended) = self
            .sandbox_for(
                backend,
                settings,
                requester.project,
                agent,
                dir,
                tools,
                logins,
            )
            .await?;
        let sandbox = &agent_box.sandbox;
        let (route, env) = self
            .route(requester, login_hosts, provider, Some(sandbox))
            .await?;
        let forwarder = sandbox.forwarder();
        Ok(Placed {
            wrap: sandbox.wrap(),
            expose: Expose::new(move |addr| {
                let forwarder = forwarder.clone();
                async move { forwarder.forward(addr).await.map_err(std::io::Error::other) }
            }),
            state: sandbox.home().join("d").join(short(driver)),
            tmp: Some(sandbox.tmp()),
            env,
            contained: true,
            ended,
            _route: route,
        })
    }

    /// The agent's sandbox over `dir`: the one it has, or a new one, replacing one over another
    /// directory or one that died. Returns the processes that ended with a replaced one.
    #[allow(clippy::too_many_arguments)]
    async fn sandbox_for(
        &self,
        backend: &Arc<dyn Backend>,
        settings: &DaemonSettings,
        project: ProjectId,
        agent: AgentId,
        dir: &Path,
        tools: Option<SocketAddr>,
        logins: Vec<PathBuf>,
    ) -> Result<(Arc<AgentBox>, Vec<ProcessName>), String> {
        let mut boxes = self.boxes.lock().await;
        if let Some(b) = boxes.get(&agent)
            && b.sandbox.worktree() == dir
            && b.sandbox.is_alive()
        {
            return Ok((Arc::clone(b), Vec::new()));
        }
        let ended = match boxes.remove(&agent) {
            Some(old) => self.destroy(old).await,
            None => Vec::new(),
        };
        let name = format!("{}-{}", self.containment.tag, short(agent));
        let mut paths = Vec::new();
        if dir.join(".git").exists() {
            paths.extend(x100_sandbox::git_paths(dir).map_err(|e| e.to_string())?);
        }
        paths.extend(
            logins
                .into_iter()
                .filter(|l| l.is_dir())
                .map(Mount::read_write),
        );
        if let Some((_, cert)) = &self.containment.proxy {
            paths.push(Mount::read_only(cert));
        }
        let mut programs = self.programs.read().expect("programs lock").clone();
        programs.extend(settings.sandbox_programs.iter().cloned());
        programs.retain(|p| p.exists());
        programs.sort();
        programs.dedup();
        let spec = SandboxSpec {
            dir: self.containment.dir.join(short(agent)),
            name: name.clone(),
            worktree: dir.to_path_buf(),
            programs,
            paths,
            endpoints: tools.into_iter().collect(),
        };
        let sandbox = backend
            .create(spec)
            .await
            .map_err(|e| format!("the agent's sandbox could not be made: {e}"))?;
        if let Err(e) = self.registry.record_named(&key(&name), sandbox.anchor()) {
            eprintln!("x100: could not write down sandbox {name}'s process: {e}");
        }
        let made = Arc::new(AgentBox {
            project,
            name,
            sandbox,
            processes: Mutex::new(HashMap::new()),
        });
        boxes.insert(agent, Arc::clone(&made));
        Ok((made, ended))
    }

    /// A route on the proxy for `requester`, mapped into `sandbox` when there is one, and the
    /// environment that sends a program's traffic through it.
    async fn route(
        &self,
        requester: Requester,
        login_hosts: &[String],
        provider: &[String],
        sandbox: Option<&Sandbox>,
    ) -> Result<(Option<Route>, Vec<(String, String)>), String> {
        let Some((proxy, cert)) = &self.containment.proxy else {
            return Ok((None, Vec::new()));
        };
        let hosts: Vec<&str> = login_hosts.iter().map(String::as_str).collect();
        let provider: Vec<&str> = provider.iter().map(String::as_str).collect();
        let route = proxy
            .open_route(requester, &hosts, &provider)
            .await
            .map_err(|e| format!("could not open a route on the proxy: {e}"))?;
        if let Some(sandbox) = sandbox {
            sandbox
                .forward(route.addr())
                .await
                .map_err(|e| e.to_string())?;
        }
        let env = x100_vault::sandbox_env(&route.url(), cert, NO_PROXY);
        Ok((Some(route), env))
    }

    /// Starts `command` in the agent's sandbox, owned by the sandbox, its output appended to a
    /// file inside that the agent can read. Returns that file.
    pub async fn start_process(
        &self,
        requester: Requester,
        name: &ProcessName,
        command: &str,
    ) -> Result<PathBuf, String> {
        let agent_box = self.boxes.lock().await.get(&requester.agent).cloned();
        let Some(agent_box) = agent_box.filter(|b| b.sandbox.is_alive()) else {
            return Err(
                "you have no sandbox to run a process in: this daemon runs drivers \
                        without one"
                    .into(),
            );
        };
        let sandbox = &agent_box.sandbox;
        let logs = sandbox.home().join("processes");
        std::fs::create_dir_all(&logs).map_err(|e| format!("making {}: {e}", logs.display()))?;
        let log = logs.join(format!("{}.log", name.0));
        let (route, mut env) = self.route(requester, &[], &[], Some(sandbox)).await?;
        env.extend(
            ["PATH", "LANG"]
                .into_iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_owned(), v))),
        );
        let line = CommandLine {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!("exec >>{} 2>&1; {command}", shell_quote(&log)).into(),
            ],
        };
        let id = sandbox
            .start_process(line, env)
            .await
            .map_err(|e| format!("the process could not start: {e}"))?;
        agent_box
            .processes
            .lock()
            .expect("processes lock")
            .insert(name.clone(), (id, route));
        Ok(log)
    }

    /// Stops a process `start_process` started, with everything it started.
    pub async fn stop_process(&self, agent: AgentId, name: &ProcessName) -> Result<(), String> {
        let agent_box = self.boxes.lock().await.get(&agent).cloned();
        let found = agent_box.as_ref().and_then(|b| {
            b.processes
                .lock()
                .expect("processes lock")
                .remove(name)
                .map(|(id, route)| (id, route, b))
        });
        let Some((id, _route, b)) = found else {
            // Its sandbox went with the daemon or with the project's setting up: nothing runs.
            return Ok(());
        };
        match b.sandbox.stop_process(id).await {
            Ok(()) | Err(x100_sandbox::Error::NoProcess(_)) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Destroys every sandbox of `project`, returning the processes that ended with each agent's.
    pub async fn destroy_project(&self, project: ProjectId) -> Vec<(AgentId, Vec<ProcessName>)> {
        let gone: Vec<(AgentId, Arc<AgentBox>)> = {
            let mut boxes = self.boxes.lock().await;
            let agents: Vec<AgentId> = boxes
                .iter()
                .filter(|(_, b)| b.project == project)
                .map(|(a, _)| *a)
                .collect();
            agents
                .into_iter()
                .filter_map(|a| boxes.remove(&a).map(|b| (a, b)))
                .collect()
        };
        let mut ended = Vec::new();
        for (agent, b) in gone {
            ended.push((agent, self.destroy(b).await));
        }
        ended
    }

    async fn destroy(&self, b: Arc<AgentBox>) -> Vec<ProcessName> {
        let mut names: Vec<ProcessName> = b
            .processes
            .lock()
            .expect("processes lock")
            .drain()
            .map(|(n, _)| n)
            .collect();
        names.sort();
        let name = key(&b.name);
        // Every driver of the agent has ended, so nothing else holds the sandbox.
        match Arc::try_unwrap(b) {
            Ok(b) => {
                if let Err(e) = b.sandbox.destroy().await {
                    eprintln!("x100: a sandbox could not be removed: {e}");
                }
            }
            Err(b) => drop(b),
        }
        let _ = self.registry.forget_named(&name);
        names
    }
}

/// How the process list names a sandbox's anchor.
fn key(name: &str) -> String {
    format!("sandbox {name}")
}

/// The last ten characters of an id: unique enough within a daemon, and short, since socket
/// paths are made under the directories these name.
fn short(id: impl std::fmt::Display) -> String {
    let s = id.to_string().to_ascii_lowercase();
    s[s.len().saturating_sub(10)..].to_owned()
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
