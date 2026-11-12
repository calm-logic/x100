//! How records read on screen, in the words `PRODUCT.md` uses.

use x100_core::action::{AskMeAbout, Risk};
use x100_core::agent::{AgentState, Waiting};
use x100_core::driver::{ContextFill, DriverState};
use x100_core::effort::Effort;
use x100_core::fold::DaemonState;
use x100_core::harness::HarnessKind;
use x100_core::id::{HarnessId, MachineId};
use x100_core::money::Money;
use x100_core::spec::SpecKind;
use x100_core::time::Timestamp;

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
        Some(HarnessKind::ClaudeCodeTerminal) => "Claude Code, terminal".into(),
        Some(HarnessKind::CodexTerminal) => "Codex, terminal".into(),
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

/// Dollars and cents. Spend that rounds to no cents is shown as under a cent, not as nothing: a
/// small task on a cheap model costs a fraction of one.
pub fn money(m: Money) -> String {
    let cents = (m.micros() + 5_000) / 10_000;
    if cents == 0 && m.micros() > 0 {
        return "<$0.01".into();
    }
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

/// What a project is called where it is listed: its name, or "New project" until its CEO sets it
/// up.
pub fn project_name(p: Option<&x100_core::fold::ProjectState>) -> String {
    p.and_then(|p| p.project())
        .and_then(|p| p.name())
        .unwrap_or("New project")
        .to_owned()
}

pub fn agents(n: usize) -> String {
    if n == 1 {
        "1 agent".into()
    } else {
        format!("{n} agents")
    }
}

/// The three specs, as the Specs tab names them.
pub fn spec_name(kind: SpecKind) -> &'static str {
    match kind {
        SpecKind::Brd => "Business requirements",
        SpecKind::Prd => "Product requirements",
        SpecKind::Add => "Architecture design",
    }
}

pub fn spec_short(kind: SpecKind) -> &'static str {
    match kind {
        SpecKind::Brd => "Business",
        SpecKind::Prd => "Product",
        SpecKind::Add => "Tech",
    }
}

pub fn spec_abbreviation(kind: SpecKind) -> &'static str {
    match kind {
        SpecKind::Brd => "BRD",
        SpecKind::Prd => "PRD",
        SpecKind::Add => "ADD",
    }
}

pub fn ask_me_about(level: AskMeAbout) -> &'static str {
    match level {
        AskMeAbout::Nothing => "Nothing",
        AskMeAbout::HighRiskOnly => "High risk only",
        AskMeAbout::MediumAndUp => "Medium and up",
        AskMeAbout::EveryDecision => "Every decision",
    }
}

pub fn risk(risk: Risk) -> &'static str {
    match risk {
        Risk::Low => "Low risk",
        Risk::Medium => "Medium risk",
        Risk::High => "High risk",
    }
}

/// An amount the builder typed, in dollars: `50`, `12.5`, `$12.50`.
pub fn parse_money(text: &str) -> Option<Money> {
    let text = text.trim().trim_start_matches('$').replace(',', "");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    if whole.is_empty() && fraction.is_empty() || fraction.len() > 6 {
        return None;
    }
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if !digits(whole) || !digits(fraction) {
        return None;
    }
    let dollars: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let micros: u64 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<6}").parse().ok()?
    };
    dollars
        .checked_mul(1_000_000)?
        .checked_add(micros)
        .map(Money::from_micros)
}

/// Tokens in thousands and millions.
pub fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// A moment as `12 Nov 09:00:03`, in UTC: the log's clock is the daemon's, and a machine's time
/// zone is not known to it.
pub fn moment(at: Timestamp) -> String {
    let secs = at.as_millis() / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{day} {} {:02}:{:02}:{:02}",
        MONTHS[(month - 1) as usize],
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_round_to_the_cent() {
        assert_eq!(money(Money::ZERO), "$0.00");
        assert_eq!(money(Money::from_micros(4_000)), "<$0.01");
        assert_eq!(money(Money::from_micros(42_000)), "$0.04");
        assert_eq!(money(Money::from_micros(45_000)), "$0.05");
        assert_eq!(money(Money::from_dollars(15)), "$15.00");
    }

    #[test]
    fn typed_amounts_and_moments() {
        assert_eq!(parse_money("$12.50"), Some(Money::from_micros(12_500_000)));
        assert_eq!(parse_money("50"), Some(Money::from_dollars(50)));
        assert_eq!(parse_money("0.000001"), Some(Money::from_micros(1)));
        assert_eq!(parse_money("1,000"), Some(Money::from_dollars(1000)));
        assert_eq!(parse_money(""), None);
        assert_eq!(parse_money("ten"), None);
        assert_eq!(parse_money("1.2.3"), None);
        assert_eq!(
            moment(Timestamp::from_millis(1_794_387_603_000)),
            "11 Nov 09:00:03"
        );
        assert_eq!(moment(Timestamp::EPOCH), "1 Jan 00:00:00");
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
