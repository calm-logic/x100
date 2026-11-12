//! How records read on screen, in the words `PRODUCT.md` uses.

use x100_core::agent::{AgentState, Waiting};
use x100_core::driver::{ContextFill, DriverState};
use x100_core::effort::Effort;
use x100_core::fold::DaemonState;
use x100_core::harness::HarnessKind;
use x100_core::id::{HarnessId, MachineId};
use x100_core::money::Money;

pub fn effort(e: Effort) -> &'static str {
    match e {
        Effort::Low => "Low",
        Effort::Medium => "Medium",
        Effort::High => "High",
        Effort::Max => "Max",
    }
}

pub fn harness(daemon: &DaemonState, id: HarnessId) -> String {
    match daemon.harness(id).map(|h| &h.kind) {
        Some(HarnessKind::ClaudeCode) => "Claude Code".into(),
        Some(HarnessKind::Codex) => "Codex".into(),
        Some(HarnessKind::Custom { name }) => name.clone(),
        None => "Unknown harness".into(),
    }
}

pub fn machine(daemon: &DaemonState, id: MachineId) -> String {
    daemon
        .machines()
        .find(|m| m.id == id)
        .map_or_else(|| "Unknown machine".into(), |m| m.name.clone())
}

/// What an agent is doing, when it has no task to name.
pub fn agent_state(state: &AgentState) -> &'static str {
    match state {
        AgentState::Planned => "Not started",
        AgentState::Working => "Working",
        AgentState::Waiting(Waiting::OnWaits) => "Waiting",
        AgentState::Waiting(Waiting::ForDriver(_)) => "Waiting for a driver",
        AgentState::Idle => "Idle",
        AgentState::Retired => "Retired",
    }
}

pub fn driver_state(state: &DriverState) -> &'static str {
    match state {
        DriverState::Starting => "Starting",
        DriverState::Running => "Running",
        DriverState::Warm => "Warm",
        DriverState::Compacting => "Compacting",
        DriverState::HandingOff(_) => "Handing off",
        DriverState::Ended(_) => "Ended",
        DriverState::Failed(_) => "Failed",
    }
}

pub fn money(m: Money) -> String {
    let cents = (m.micros() + 5_000) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

pub fn context(fill: Option<ContextFill>) -> String {
    match fill {
        Some(f) if f.window > 0 => format!("{}%", f.used * 100 / f.window),
        _ => "-".into(),
    }
}

pub fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{} KB", bytes.div_ceil(1_000)),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

/// A file's kind, from its media type.
pub fn kind(media_type: &str) -> String {
    let (top, sub) = media_type.split_once('/').unwrap_or((media_type, ""));
    match (top, sub) {
        ("image", _) => "Image".into(),
        (_, "pdf") => "PDF".into(),
        (_, "markdown") => "Markdown".into(),
        ("text", _) => "Text".into(),
        (_, "json") => "JSON".into(),
        (_, "zip") => "Archive".into(),
        _ if sub.is_empty() => "File".into(),
        _ => sub.to_uppercase(),
    }
}

/// A media type for a file the builder attaches, from its extension.
pub fn media_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("pdf") => "application/pdf",
        Some("md") => "text/markdown",
        Some("txt" | "log") => "text/plain",
        Some("csv") => "text/csv",
        Some("html" | "htm") => "text/html",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

pub fn agents(n: usize) -> String {
    if n == 1 {
        "1 agent".into()
    } else {
        format!("{n} agents")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_round_to_the_cent() {
        assert_eq!(money(Money::ZERO), "$0.00");
        assert_eq!(money(Money::from_micros(42_000)), "$0.04");
        assert_eq!(money(Money::from_micros(45_000)), "$0.05");
        assert_eq!(money(Money::from_dollars(15)), "$15.00");
    }

    #[test]
    fn sizes_and_kinds() {
        assert_eq!(size(48_213), "49 KB");
        assert_eq!(size(1_900_000), "1.9 MB");
        assert_eq!(kind("image/png"), "Image");
        assert_eq!(kind("application/pdf"), "PDF");
        assert_eq!(media_type("Sketch.PNG"), "image/png");
    }
}
