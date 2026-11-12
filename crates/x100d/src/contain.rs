//! What contains drivers on this machine: the sandbox backend, tried once at start so a machine
//! that cannot make a sandbox says why before any driver is started, and the programs every
//! sandbox shows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use x100_sandbox::{Backend, Bubblewrap, Container, Runtime, SandboxSpec};

use crate::config::{Config, SandboxKind};

/// Where sandboxes keep their own directories, under the data directory. Short, since a Codex
/// driver's socket is made under it.
pub fn dir(config: &Config) -> PathBuf {
    config.data_dir.join("sb")
}

/// Unique to this daemon's data directory, which no other daemon can hold at the same time: a
/// sandbox's name starts with it, so one daemon's leftovers are told from another's.
pub fn tag(config: &Config) -> String {
    let path = config.data_dir.display().to_string();
    // FNV-1a: a stable hash with no dependency, for a name, not for security.
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("x{:08x}", hash as u32)
}

/// The backend this machine can make sandboxes with, or why it cannot: it makes one and destroys
/// it. Before that, whatever a daemon killed on this data directory left is removed: the
/// directories of its sandboxes, and the containers of a container backend, whose client's death
/// does not stop them. A bubblewrap sandbox's anchor was reaped with the drivers.
pub async fn backend(config: &Config) -> Result<Arc<dyn Backend>, String> {
    let base = dir(config);
    let _ = std::fs::remove_dir_all(&base);
    let helper = match &config.sandbox_helper {
        Some(h) => h.clone(),
        None => std::env::current_exe()
            .map_err(|e| format!("cannot find the sandbox's helper: {e}"))?
            .with_file_name("x100d-sandbox"),
    };
    if !helper.is_file() {
        return Err(format!(
            "the sandbox's helper {} is missing",
            helper.display()
        ));
    }
    let backend: Arc<dyn Backend> = match config.sandbox {
        SandboxKind::Bubblewrap => Arc::new(Bubblewrap::new(&helper)),
        SandboxKind::Docker | SandboxKind::Podman => {
            let runtime = match config.sandbox {
                SandboxKind::Podman => Runtime::podman(),
                _ => Runtime::docker(),
            };
            let container = Container::new(runtime, &helper);
            container
                .kill_leftovers(&format!("{}-", tag(config)))
                .await
                .map_err(|e| e.to_string())?;
            container.ensure_image().await.map_err(|e| e.to_string())?;
            Arc::new(container)
        }
    };
    probe(&*backend, config, &base).await?;
    Ok(backend)
}

async fn probe(backend: &dyn Backend, config: &Config, base: &Path) -> Result<(), String> {
    let work = base.join("probe-work");
    std::fs::create_dir_all(&work).map_err(|e| format!("making {}: {e}", work.display()))?;
    let spec = SandboxSpec {
        name: format!("{}-probe", tag(config)),
        dir: base.join("probe"),
        worktree: work.clone(),
        programs: Vec::new(),
        paths: Vec::new(),
        endpoints: Vec::new(),
    };
    let made = backend.create(spec).await;
    let _ = std::fs::remove_dir_all(&work);
    let sandbox = made.map_err(|e| e.to_string())?;
    sandbox.destroy().await.map_err(|e| e.to_string())
}

/// What a sandbox shows read-only so drivers can run: each program's install, found on the
/// daemon's search path, and the Rust toolchain's parts. Never the whole of `~/.cargo`, which can
/// hold a registry token.
pub fn programs(names: &[&str], extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = names
        .iter()
        .filter_map(|n| on_path(n))
        .flat_map(|p| x100_sandbox::program_paths(&p))
        .collect();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        out.extend(
            [".cargo/bin", ".cargo/registry", ".cargo/git", ".rustup"]
                .iter()
                .map(|d| home.join(d))
                .filter(|d| d.is_dir()),
        );
    }
    out.extend(extra.iter().filter(|p| p.exists()).cloned());
    out.sort();
    out.dedup();
    out
}

pub fn on_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}
