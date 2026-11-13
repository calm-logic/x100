//! An agent's transcript written as a terminal log, and the words for its drivers: what each was,
//! why it started and why it ended. The log is plain text with SGR colours, fed to the same
//! terminal widget a terminal driver's screen is, so both look alike.

use std::collections::HashSet;

use x100_core::driver::{
    Driver, DriverState, EndReason, Failure, HandoffCause, PlannedReason, RetireReason,
    StartReason, StopReason, TakeoverCause,
};
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::money::Usage;
use x100_core::transcript::{Item, TurnEnd};

use crate::words;

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// How a tool's input or result is shortened to one line.
const BRIEF: usize = 90;

/// "Claude Code · sonnet · Medium".
pub fn driver_label(daemon: &DaemonState, d: &Driver) -> String {
    format!(
        "{} · {} · {}",
        words::harness(daemon, d.harness),
        d.model.0,
        words::effort(d.effort)
    )
}

/// Why a driver ended, or `None` while it is live.
pub fn ended(state: &DriverState) -> Option<String> {
    Some(match state {
        DriverState::Ended(EndReason::Retired(r)) => format!("retired, {}", retired(*r)),
        DriverState::Ended(EndReason::HandedOff(p)) => format!("handed off, {}", planned(*p)),
        DriverState::Ended(EndReason::Stopped(s)) => format!("stopped, {}", stopped(*s)),
        DriverState::Failed(f) => format!("failed, {}", failure(f)),
        _ => return None,
    })
}

fn retired(r: RetireReason) -> &'static str {
    match r {
        RetireReason::ContextFull => "its context was full",
        RetireReason::TooManyTasks => "it had done its share of tasks",
        RetireReason::WarmTooLong => "warm too long",
        RetireReason::TooOld => "it had run too long",
        RetireReason::CannotCompact => "it could not compact",
        RetireReason::AgentRetired => "its agent retired",
        RetireReason::Unfit(why) => crate::wording::unfit(why),
        RetireReason::ProjectSetUp => "the project was set up",
    }
}

fn planned(p: PlannedReason) -> &'static str {
    match p {
        PlannedReason::ContextFull => "its context filled mid-task",
        PlannedReason::EffortChanged => "the task's effort changed",
        PlannedReason::Disallowed => "the builder turned off its harness or model",
        PlannedReason::RangeNarrowed => "the agent's effort range was narrowed",
    }
}

fn stopped(s: StopReason) -> &'static str {
    match s {
        StopReason::ReplacedByBuilder => "replaced by the builder",
        StopReason::DaemonRestarted => "the daemon restarted",
        StopReason::LabRunEnded => "its lab run ended",
    }
}

fn failure(f: &Failure) -> String {
    match f {
        Failure::ProcessExited { code: Some(c) } => format!("its process exited with {c}"),
        Failure::ProcessExited { code: None } => "its process was killed".into(),
        Failure::StreamBroke => "its output broke off".into(),
        Failure::Error { detail, .. } => detail.clone(),
        Failure::Stalled { silent_for } => format!("silent for {}s", silent_for.as_secs()),
        Failure::LastTurnFailed => "its last turn before a handoff failed".into(),
        Failure::DaemonRestarted => "the daemon restarted mid-turn".into(),
    }
}

/// Why the old driver went, as the new one records it.
pub fn handoff_cause(cause: &HandoffCause) -> String {
    match cause {
        HandoffCause::Takeover(TakeoverCause::Failed(f)) => format!("failed, {}", failure(f)),
        HandoffCause::Takeover(TakeoverCause::Stopped(s)) => format!("stopped, {}", stopped(*s)),
        HandoffCause::Planned(p) => format!("handed off, {}", planned(*p)),
    }
}

/// What a driver was started for.
pub fn started_for(p: &ProjectState, why: &StartReason) -> String {
    match why {
        StartReason::NewTask { task } => match p.task(*task) {
            Some(t) => format!("for the task “{}”", t.task.title),
            None => "for a new task".into(),
        },
        StartReason::Wake => "woken by its inbox".into(),
        StartReason::Handoff(_) => "to carry on the work".into(),
    }
}

/// The line that marks a change of driver: which took over and why the last one ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Counting the agent's drivers from 1.
    pub number: usize,
    pub label: String,
    pub started_for: String,
    /// The one before, by number, and why it ended.
    pub previous: Option<(usize, String)>,
}

impl Change {
    pub fn text(&self) -> String {
        match &self.previous {
            None => format!(
                "Driver {} started: {}, {}.",
                self.number, self.label, self.started_for
            ),
            Some((n, why)) => format!(
                "Driver {} took over: {}, {}. Driver {n} ended: {why}.",
                self.number, self.label, self.started_for
            ),
        }
    }

    /// The marker as a log line.
    pub fn ansi(&self) -> String {
        format!("{YELLOW}━━ {}{RESET}\r\n", self.text())
    }
}

/// Turns transcript items into log text, remembering which tool calls showed output live.
#[derive(Default)]
pub struct LogWriter {
    live_output: HashSet<String>,
    /// Inside a tool's live output, which is shown as it arrives with no line of its own.
    in_output: bool,
}

impl LogWriter {
    pub fn item(&mut self, item: &Item) -> String {
        let mut out = String::new();
        if self.in_output && !matches!(item, Item::ToolOutput { .. }) {
            out.push_str("\r\n");
            self.in_output = false;
        }
        match item {
            Item::StartPack { .. } | Item::DriverChanged { .. } => {}
            Item::TurnStarted => out.push_str(&format!("{DIM}▸ turn started{RESET}\r\n")),
            Item::Text { text } => out.push_str(&format!(
                "{BOLD}●{RESET} {}\r\n",
                lines(text.trim_end(), "  ")
            )),
            Item::ToolCall { name, input, .. } => out.push_str(&format!(
                "{CYAN}▸ {name}{RESET} {DIM}{}{RESET}\r\n",
                brief_input(input)
            )),
            Item::ToolOutput { call, chunk } => {
                if !self.in_output {
                    out.push_str("    ");
                }
                self.live_output.insert(call.clone());
                self.in_output = true;
                out.push_str(&chunk.replace("\r\n", "\n").replace('\n', "\r\n    "));
            }
            Item::ToolResult {
                call,
                output,
                is_error,
            } => {
                let (mark, colour) = if *is_error {
                    ("✗", RED)
                } else {
                    ("✓", GREEN)
                };
                let shown = if self.live_output.remove(call) {
                    String::new()
                } else {
                    brief_output(output)
                };
                out.push_str(&format!("  {colour}{mark}{RESET} {shown}\r\n"));
            }
            Item::Usage { usage } => {
                out.push_str(&format!("  {DIM}usage: {}{RESET}\r\n", usage_text(usage)))
            }
            Item::TurnEnded { how } => out.push_str(&match how {
                TurnEnd::Completed => format!("{DIM}── turn ended ──{RESET}\r\n\r\n"),
                TurnEnd::Failed { detail } => {
                    format!("{RED}── turn failed: {} ──{RESET}\r\n\r\n", clip(detail))
                }
                TurnEnd::Interrupted => format!("{YELLOW}── turn interrupted ──{RESET}\r\n\r\n"),
            }),
        }
        out
    }
}

/// A plain line the view adds itself, such as why a terminal cannot be shown.
pub fn notice(text: &str) -> String {
    format!("{DIM}{text}{RESET}\r\n")
}

fn lines(text: &str, indent: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\n', &format!("\r\n{indent}"))
}

fn clip(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > BRIEF {
        format!("{}…", line.chars().take(BRIEF).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// The fields that say what a call is about, most telling first: a daemon tool's input is a
/// nested object, and its JSON in full would fill the line with ids.
const TELLING: [&str; 8] = [
    "command", "path", "summary", "title", "note", "body", "error", "message",
];

/// The first telling string field anywhere in `value`.
fn telling(value: &serde_json::Value) -> Option<&str> {
    let serde_json::Value::Object(map) = value else {
        return None;
    };
    TELLING
        .iter()
        .find_map(|key| map.get(*key)?.as_str())
        .or_else(|| map.values().find_map(telling))
}

/// A tool's input in one line: what it is about, or else its JSON.
fn brief_input(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::String(s) => clip(s),
        serde_json::Value::Object(map) if map.len() == 1 => match map.values().next() {
            Some(serde_json::Value::String(s)) => clip(s),
            _ => clip(telling(input).unwrap_or(&input.to_string())),
        },
        _ => clip(telling(input).unwrap_or(&input.to_string())),
    }
}

/// A result in one line. A daemon tool answers in JSON meant for the model, so only a telling
/// field of it is shown.
fn brief_output(output: &str) -> String {
    if let Ok(json @ serde_json::Value::Object(_)) = serde_json::from_str(output) {
        return telling(&json).map_or_else(|| format!("{DIM}done{RESET}"), clip);
    }
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    match lines.as_slice() {
        [] => format!("{DIM}no output{RESET}"),
        [one] => clip(one),
        [first, rest @ ..] => format!("{} {DIM}(+{} lines){RESET}", clip(first), rest.len()),
    }
}

fn usage_text(u: &Usage) -> String {
    let mut parts = vec![
        format!("{} in", thousands(u.input)),
        format!("{} out", thousands(u.output)),
    ];
    if u.cache_read > 0 {
        parts.push(format!("{} cached", thousands(u.cache_read)));
    }
    let written = u.cache_write_short + u.cache_write_long;
    if written > 0 {
        parts.push(format!("{} to cache", thousands(written)));
    }
    parts.join(" · ")
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> String {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                (false, '\r') => {}
                (false, c) => out.push(c),
            }
        }
        out
    }

    #[test]
    fn a_turn_reads_as_a_log() {
        let mut w = LogWriter::default();
        let items = [
            Item::TurnStarted,
            Item::Text {
                text: "Writing the schema.\nOne table.".into(),
            },
            Item::ToolCall {
                call: "c1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "cargo test"}),
            },
            Item::ToolOutput {
                call: "c1".into(),
                chunk: "running 3 tests\n".into(),
            },
            Item::ToolOutput {
                call: "c1".into(),
                chunk: "ok".into(),
            },
            Item::ToolResult {
                call: "c1".into(),
                output: "running 3 tests\nok".into(),
                is_error: false,
            },
            Item::ToolCall {
                call: "c2".into(),
                name: "report".into(),
                input: serde_json::json!({"tries": 2}),
            },
            Item::ToolResult {
                call: "c2".into(),
                output: "refused\nno such task".into(),
                is_error: true,
            },
            Item::Usage {
                usage: Usage {
                    input: 4200,
                    output: 900,
                    cache_read: 12_000,
                    ..Usage::default()
                },
            },
            Item::TurnEnded {
                how: TurnEnd::Completed,
            },
        ];
        let text: String = items.iter().map(|i| plain(&w.item(i))).collect();
        assert_eq!(
            text,
            "▸ turn started\n\
             ● Writing the schema.\n  One table.\n\
             ▸ bash cargo test\n\
             \x20   running 3 tests\n    ok\n\
             \x20 ✓ \n\
             ▸ report {\"tries\":2}\n\
             \x20 ✗ refused (+1 lines)\n\
             \x20 usage: 4,200 in · 900 out · 12,000 cached\n\
             ── turn ended ──\n\n"
        );
    }

    #[test]
    fn a_change_of_driver_names_both() {
        let change = Change {
            number: 2,
            label: "fake-fernleaf · fake-brief · Low".into(),
            started_for: "woken by its inbox".into(),
            previous: Some((1, "retired, warm too long".into())),
        };
        assert_eq!(
            change.text(),
            "Driver 2 took over: fake-fernleaf · fake-brief · Low, woken by its inbox. \
             Driver 1 ended: retired, warm too long."
        );
    }
}
