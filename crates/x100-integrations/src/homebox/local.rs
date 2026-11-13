//! A Homebox on this machine, found and given a key through Docker.
//!
//! Homebox has no designed local access: loopback is not trusted, and its only way for a program
//! is an `hbk_` key. What exists is Docker: a user who may run `docker` can run Homebox's own
//! code inside its container. So "use local" is two things behind [`LocalHomebox`]: finding the
//! `homebox-admin` container's published port, which only reads, and minting an edit key named
//! for x100 with one fixed command in that container, once, after the builder confirms. The key
//! goes straight to the vault. A supported `mint-key` command in Homebox would replace the
//! script below.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use x100_vault::SecretValue;

/// The container Homebox's admin app runs in.
pub const CONTAINER: &str = "homebox-admin";

/// The port the admin app listens on inside its container, which Docker publishes on loopback.
const INNER_PORT: &str = "8000/tcp";

/// Mints an edit key named `x100` with Homebox's own code and prints only the key. It uses the
/// same token maker, model and database session as `POST /api/identities`
/// (`app/routes/identities.py`), and imports nothing that runs the app's start-up.
pub const MINT_SCRIPT: &str = "import asyncio
from app.db import SessionLocal, engine
from app.models import Identity
from app.routes.identities import _new_token
async def main():
    tok, h, prefix = _new_token()
    async with SessionLocal() as s:
        s.add(Identity(kind=\"token\", name=\"x100\", email=None, enabled=True, token_hash=h, token_prefix=prefix, role=\"edit\"))
        await s.commit()
    await engine.dispose()
    print(tok)
asyncio.run(main())";

/// The whole command, as the program and its arguments after `docker`.
pub fn mint_args() -> Vec<&'static str> {
    vec!["exec", "-w", "/srv", CONTAINER, "python", "-c", MINT_SCRIPT]
}

/// How long a Docker command may take.
const WITHIN: Duration = Duration::from_secs(30);

#[async_trait]
pub trait LocalHomebox: Send + Sync {
    /// The loopback port the container's admin app is published on, if the container is there.
    /// Only reads.
    async fn port(&self) -> Option<u16>;

    /// Mints an edit key for x100 in the container. Runs once per connect, after the builder
    /// confirmed. The key is in the value returned and nowhere else.
    async fn mint(&self) -> Result<SecretValue, String>;
}

/// The real one: the `docker` command line.
pub struct Docker {
    pub program: PathBuf,
}

#[async_trait]
impl LocalHomebox for Docker {
    async fn port(&self) -> Option<u16> {
        let out = run(&self.program, &["port", CONTAINER, INNER_PORT])
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        published_port(&String::from_utf8_lossy(&out.stdout))
    }

    async fn mint(&self) -> Result<SecretValue, String> {
        let mut out = run(&self.program, &mint_args()).await?;
        // The key goes straight into a value that wipes itself, and the output it came in is
        // wiped too. Stdout is never quoted, so an error cannot carry it.
        let printed = out.stdout.trim_ascii();
        let looks_right = printed.starts_with(b"hbk_") && printed.len() > 12;
        let key = SecretValue::new(printed.to_vec());
        out.stdout.fill(0);
        std::hint::black_box(&out.stdout);
        if !out.status.success() {
            return Err(format!(
                "minting a key in {CONTAINER} failed ({}): {}",
                out.status,
                crate::api::short(&String::from_utf8_lossy(&out.stderr))
            ));
        }
        if !looks_right {
            return Err(format!(
                "minting a key in {CONTAINER} printed something that is not a key"
            ));
        }
        Ok(key)
    }
}

async fn run(program: &std::path::Path, args: &[&str]) -> Result<std::process::Output, String> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(WITHIN, child).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("{} could not be run: {e}", program.display())),
        Err(_) => Err(format!("{} did not finish in time", program.display())),
    }
}

/// The loopback port in `docker port`'s answer, such as `127.0.0.1:7765`. A port published on
/// every address counts too, since loopback is among them.
fn published_port(out: &str) -> Option<u16> {
    out.lines().find_map(|line| {
        let (host, port) = line.trim().rsplit_once(':')?;
        let local = matches!(host, "127.0.0.1" | "0.0.0.0" | "[::1]" | "[::]" | "::");
        local.then(|| port.parse().ok()).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_loopback_port_is_read() {
        assert_eq!(published_port("127.0.0.1:7765\n"), Some(7765));
        assert_eq!(published_port("0.0.0.0:9000\n[::]:9000\n"), Some(9000));
        assert_eq!(published_port("192.168.1.2:7765\n"), None);
        assert_eq!(published_port(""), None);
    }

    #[test]
    fn the_mint_command_is_fixed() {
        let args = mint_args();
        assert_eq!(
            &args[..6],
            ["exec", "-w", "/srv", CONTAINER, "python", "-c"]
        );
        assert!(MINT_SCRIPT.contains("role=\"edit\"") && MINT_SCRIPT.contains("name=\"x100\""));
    }
}
