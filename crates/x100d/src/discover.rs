//! Finding the harnesses on this machine and recording them in the daemon's log.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_core::event::DaemonEvent;
use x100_core::harness::{Harness, Machine};
use x100_core::id::{HarnessId, MachineId};
use x100_core::party::Actor;
use x100_core::time::Clock;
use x100_drivers::claude::{ClaudeCode, ClaudeCodeConfig};
use x100_drivers::codex::{Codex, CodexConfig};
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

pub fn discover(
    config: &Config,
    log: &DaemonLog,
    tools: Arc<dyn DaemonTools>,
) -> Result<Found, Error> {
    let candidates = match &config.scripts_dir {
        Some(dir) => fakes(config, dir, tools)?,
        None => real(config),
    };
    let mut found = HashMap::new();
    // Appended together after the loop, so a crash during discovery leaves the log as it was.
    let mut events = Vec::new();
    for harness in candidates {
        let record = Harness {
            // Replaced below by the id the log already has for this kind, if it has one.
            id: new_id(&*config.clock, HarnessId::new),
            kind: harness.kind(),
            support: harness.supports(),
            models: harness.models(),
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
    let machine = match log.with_state(|s| s.machines().next().map(|m| m.id)) {
        Some(id) => id,
        None => {
            let id = new_id(&*config.clock, MachineId::new);
            events.push((
                Actor::Daemon,
                DaemonEvent::MachineAdded(Machine {
                    id,
                    name: "local".into(),
                    harnesses: found.keys().copied().collect(),
                    cap: 16,
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
) -> Result<Vec<Arc<dyn x100_drivers::Harness>>, Error> {
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
            .map(|h| Arc::new(h) as Arc<dyn x100_drivers::Harness>)
            .map_err(|e| Error::Script(script, e.to_string()))
        })
        .collect()
}

/// Claude Code and Codex, when their programs are on the path. Nothing is run to find them: a
/// version that will not do is refused when a driver starts.
fn real(config: &Config) -> Vec<Arc<dyn x100_drivers::Harness>> {
    let clock: Arc<dyn Clock> = Arc::clone(&config.clock);
    let mut out: Vec<Arc<dyn x100_drivers::Harness>> = Vec::new();
    if let Some(program) = on_path("claude") {
        let mut c = ClaudeCodeConfig::new(config.data_dir.join("harnesses/claude"));
        c.program = program;
        out.push(Arc::new(ClaudeCode::new(c, Arc::clone(&clock))));
    }
    if let Some(program) = on_path("codex") {
        let mut c = CodexConfig::new(config.data_dir.join("harnesses/codex"));
        c.program = program;
        out.push(Arc::new(Codex::new(c, clock)));
    }
    out
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
