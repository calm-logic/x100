//! Finding the harnesses on this machine and recording them in the daemon's log.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_core::event::DaemonEvent;
use x100_core::harness::{Harness, HarnessKind, Login, Machine, PassedOver};
use x100_core::id::{HarnessId, MachineId};
use x100_core::party::Actor;
use x100_core::time::Clock;
use x100_drivers::claude::{ClaudeCode, ClaudeCodeConfig, ClaudeTerminal, ClaudeTerminalConfig};
use x100_drivers::codex::{Codex, CodexConfig, CodexTerminal, CodexTerminalConfig};
use x100_drivers::fake::{DaemonTools, FakeHarness};
use x100_orch::new_id;
use x100_store::DaemonLog;

use crate::config::Config;
use crate::error::Error;

/// The harnesses this run can start, by the id the daemon's log gives each, and the one machine.
pub struct Found {
    pub harnesses: HashMap<HarnessId, Arc<dyn x100_drivers::Harness>>,
    pub machine: MachineId,
}

/// A harness this run can start, and the builder's login it runs on, if it runs on one.
struct Candidate {
    harness: Arc<dyn x100_drivers::Harness>,
    login: Option<Login>,
}

pub fn discover(
    config: &Config,
    log: &DaemonLog,
    tools: Arc<dyn DaemonTools>,
) -> Result<Found, Error> {
    let (candidates, passed_over) = match &config.scripts_dir {
        Some(dir) => (fakes(config, dir, tools)?, Vec::new()),
        None => real(config),
    };
    for p in &passed_over {
        eprintln!("x100d: {:?} is not registered: {}", p.kind, p.why);
    }
    let mut found = HashMap::new();
    // Appended together after the loop, so a crash during discovery leaves the log as it was.
    let mut events = Vec::new();
    for Candidate { harness, login } in candidates {
        let record = Harness {
            // Replaced below by the id the log already has for this kind, if it has one.
            id: new_id(&*config.clock, HarnessId::new),
            kind: harness.kind(),
            support: harness.supports(),
            models: harness.models(),
            login,
        };
        let known = log.with_state(|s| s.harnesses().find(|h| h.kind == record.kind).cloned());
        let id = match known {
            None => {
                events.push((Actor::Daemon, DaemonEvent::HarnessAdded(record.clone())));
                record.id
            }
            Some(old) => {
                let record = Harness {
                    id: old.id,
                    ..record
                };
                if record != old {
                    events.push((Actor::Daemon, DaemonEvent::HarnessUpdated(record)));
                }
                old.id
            }
        };
        found.insert(id, harness);
    }
    let mut ids: Vec<HarnessId> = found.keys().copied().collect();
    ids.sort();
    let machine = match log.with_state(|s| s.machines().next().cloned()) {
        Some(m) => {
            if m.harnesses != ids || m.passed_over != passed_over {
                events.push((
                    Actor::Daemon,
                    DaemonEvent::MachineHarnessesSet {
                        machine: m.id,
                        harnesses: ids,
                        passed_over,
                    },
                ));
            }
            m.id
        }
        None => {
            let id = new_id(&*config.clock, MachineId::new);
            events.push((
                Actor::Daemon,
                DaemonEvent::MachineAdded(Machine {
                    id,
                    name: "local".into(),
                    harnesses: ids,
                    cap: 16,
                    passed_over,
                }),
            ));
            id
        }
    };
    log.append_all(events)?;
    Ok(Found {
        harnesses: found,
        machine,
    })
}

fn fakes(
    config: &Config,
    dir: &Path,
    tools: Arc<dyn DaemonTools>,
) -> Result<Vec<Candidate>, Error> {
    let program = match &config.fake_program {
        Some(p) => p.clone(),
        None => std::env::current_exe()
            .map_err(Error::Io)?
            .with_file_name("x100d-fake-harness"),
    };
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(Error::Io)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    scripts.sort();
    scripts
        .into_iter()
        .map(|script| {
            FakeHarness::new(
                &program,
                &script,
                Arc::clone(&config.clock),
                Some(Arc::clone(&tools)),
            )
            .map(|h| Candidate {
                harness: Arc::new(h),
                login: None,
            })
            .map_err(|e| Error::Script(script, e.to_string()))
        })
        .collect()
}

/// Variables a terminal driver takes from the daemon's environment besides the usual ones: a
/// proxy the builder's network needs reaches the program, as it would if the builder ran it.
const PROXY_ENV: &[&str] = &[
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
];

fn with_proxy(inherit: &mut Vec<String>) {
    inherit.extend(PROXY_ENV.iter().map(|s| (*s).to_owned()));
}

/// The login a terminal harness runs on, if its directory exists. The daemon looks only at the
/// directory, never at a file in it.
fn login(kind: HarnessKind, dir: Option<&Path>) -> Result<Login, PassedOver> {
    let passed = |why: String| PassedOver {
        kind: kind.clone(),
        why,
    };
    let dir =
        dir.ok_or_else(|| passed("no home directory is known to look for a login in".into()))?;
    if dir.is_dir() {
        Ok(Login(dir.display().to_string()))
    } else {
        Err(passed(format!(
            "no login at {}: sign in by running the program yourself once, then rescan",
            dir.display()
        )))
    }
}

/// Claude Code and Codex, when their programs are on the path: each headless, and each as its own
/// terminal program on the builder's login when that login exists. Nothing is run to find them:
/// a version that will not do is refused when a driver starts.
fn real(config: &Config) -> (Vec<Candidate>, Vec<PassedOver>) {
    let clock: Arc<dyn Clock> = Arc::clone(&config.clock);
    let harnesses = config.data_dir.join("harnesses");
    let (mut out, mut passed_over) = (Vec::new(), Vec::new());
    let mut add = |harness: Arc<dyn x100_drivers::Harness>, login| {
        out.push(Candidate { harness, login });
    };
    if let Some(program) = on_path("claude") {
        let mut c = ClaudeCodeConfig::new(harnesses.join("claude"));
        c.program = program.clone();
        add(Arc::new(ClaudeCode::new(c, Arc::clone(&clock))), None);
        match login(
            HarnessKind::ClaudeCodeTerminal,
            config.claude_login.as_deref(),
        ) {
            Ok(l) => {
                let mut c = ClaudeTerminalConfig::new(harnesses.join("claude-terminal"));
                c.program = program;
                c.base_url = config.claude_base_url.clone();
                with_proxy(&mut c.inherit_env);
                add(
                    Arc::new(ClaudeTerminal::new(c, Arc::clone(&clock))),
                    Some(l),
                );
            }
            Err(p) => passed_over.push(p),
        }
    }
    if let Some(program) = on_path("codex") {
        let mut c = CodexConfig::new(harnesses.join("codex"));
        c.program = program.clone();
        add(Arc::new(Codex::new(c, Arc::clone(&clock))), None);
        match login(HarnessKind::CodexTerminal, config.codex_login.as_deref()) {
            Ok(l) => {
                let mut c = CodexTerminalConfig::new(harnesses.join("codex-terminal"));
                c.program = program;
                c.base_url = config.codex_base_url.clone();
                with_proxy(&mut c.inherit_env);
                add(Arc::new(CodexTerminal::new(c, clock)), Some(l));
            }
            Err(p) => passed_over.push(p),
        }
    }
    (out, passed_over)
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
