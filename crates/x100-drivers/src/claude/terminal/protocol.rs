//! Driving Claude Code's own terminal program: turns are typed, and what it does is heard from its
//! hooks, its session file and the state it reports on its terminal.
//!
//! - Ready: the program has reported `idle` on its terminal, so the prompt box takes input.
//!   (`SessionStart` is not sent to `http` hooks; every other hook names the session file.)
//! - A turn: its input pasted, then Enter as a separate write. Tool calls and results come from
//!   `PreToolUse`, `PostToolUse` and `PostToolUseFailure`, text from the session file.
//! - Its end: `Stop`, once the session file holds the last message the hook names (Claude Code
//!   writes the file a moment after), or `StopFailure` with an error category. An interrupted turn fires neither,
//!   so it ends when the terminal reports the program idle again. If a hook never comes, the
//!   terminal's report ends the turn a little later, with a warning.
//! - Usage: each request's tokens in the session file, at the model's listed price.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;
use x100_core::driver::{ContextFill, Failure};
use x100_core::harness::ProviderFailure;
use x100_core::ledger::CostBasis;
use x100_core::money::{Money, Price, Usage};

use super::transcript::Transcript;
use crate::failure::{classify_claude, limit_warning};
use crate::handle::InputError;
use crate::output::{Decision, DriverOutput, PermissionId, PermissionRequest, Spent, TurnEnd};
use crate::protocol::{Effects, HookCall, Input, Key, Protocol, Ready};
use crate::terminal::pty::Screen;
use crate::terminal::{Reported, Status};

/// How long the program has to show its prompt box once started.
const START_LIMIT: Duration = Duration::from_secs(60);
/// How long a hook may lag the terminal's report that a turn is over before the report is taken.
const HOOK_GRACE: Duration = Duration::from_secs(5);
/// Claude Code tells a paste from typing by timing, so Enter follows the paste after this.
const ENTER_AFTER_PASTE: Duration = Duration::from_millis(250);
/// How often, and how many times, the session file is read for a turn's last message after `Stop`.
const ROWS_EVERY: Duration = Duration::from_millis(50);
const ROWS_TRIES: u32 = 100;
/// A typed slash command opens a menu; Enter is sent once it is drawn.
const ENTER_AFTER_COMMAND: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Turn,
    Compaction,
}

/// What a wake is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Waiting {
    Start,
    /// The terminal said turn `turn` is over; the hook that says so has not come yet.
    Hook {
        turn: u64,
        error: Option<String>,
    },
    /// `Stop` came; the session file does not yet hold the message it names.
    Rows {
        want: String,
        tries: u32,
    },
}

/// What a driver's protocol is started with.
pub(crate) struct Setup {
    pub version: String,
    pub warnings: Vec<String>,
    pub session: String,
    pub price: Price,
    pub window: u64,
    /// Where the status line command records Claude Code's status.
    pub status: PathBuf,
}

pub(crate) struct ClaudeTerminal {
    setup: Setup,
    screen: Screen,
    limits_warned: Vec<String>,
    idle: bool,
    ready: bool,
    phase: Option<Phase>,
    turn: u64,
    interrupting: bool,
    waiting: Option<Waiting>,
    transcript: Option<Transcript>,
    last_text: Option<String>,
    asked: HashMap<String, oneshot::Sender<Value>>,
    next_ask: u64,
}

impl ClaudeTerminal {
    pub(crate) fn new(setup: Setup, screen: Screen) -> Self {
        Self {
            setup,
            screen,
            limits_warned: Vec::new(),
            idle: false,
            ready: false,
            phase: None,
            turn: 0,
            interrupting: false,
            waiting: None,
            transcript: None,
            last_text: None,
            asked: HashMap::new(),
            next_ask: 0,
        }
    }

    fn wait(&mut self, waiting: Waiting, after: Duration, fx: &mut Effects) {
        self.waiting = Some(waiting);
        fx.wake_after = Some(after);
    }

    fn maybe_ready(&mut self, fx: &mut Effects) {
        if self.ready || !self.idle {
            return;
        }
        self.ready = true;
        fx.started = Some(Ok(Ready {
            version: self.setup.version.clone(),
            warnings: std::mem::take(&mut self.setup.warnings),
            session: Some(self.setup.session.clone()),
        }));
    }

    /// Emits the text written to the session file since it was last read.
    fn read_text(&mut self, fx: &mut Effects) {
        if let Some(transcript) = &mut self.transcript {
            for text in transcript.read().texts {
                self.last_text = Some(text.clone());
                fx.emit(DriverOutput::Text(text));
            }
        }
    }

    /// What the requests since the last end cost, and how full the last one left the context.
    fn spent(&mut self, fx: &mut Effects) -> Option<ContextFill> {
        self.read_text(fx);
        let requests = self
            .transcript
            .as_mut()
            .map(Transcript::take_requests)
            .unwrap_or_default();
        let mut spent = Spent {
            usage: Usage::default(),
            cost: Money::ZERO,
            basis: CostBasis::Listed,
        };
        for request in &requests {
            spent.usage += *request;
            spent.cost += self.setup.price.cost(request);
        }
        fx.emit(DriverOutput::Spent(spent));
        requests.last().map(|r| ContextFill {
            used: r.all_input() + r.output,
            window: self.setup.window,
        })
    }

    /// Warns once for each of the login's usage windows that is nearly used up.
    fn check_limits(&mut self, fx: &mut Effects) {
        let Ok(text) = std::fs::read_to_string(&self.setup.status) else {
            return;
        };
        let Ok(status) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        for window in ["five_hour", "seven_day"] {
            let limit = &status["rate_limits"][window];
            let Some(used) = limit["used_percentage"].as_f64() else {
                continue;
            };
            if self.limits_warned.iter().any(|w| w == window) {
                continue;
            }
            let name = window.replace('_', "-");
            let resets = limit["resets_at"].as_i64();
            if let Some(warning) = limit_warning("Claude Code", &name, used, resets) {
                self.limits_warned.push(window.to_owned());
                fx.emit(DriverOutput::Warning(warning));
            }
        }
    }

    fn end_turn(&mut self, fx: &mut Effects) {
        self.check_limits(fx);
        let interrupted = std::mem::take(&mut self.interrupting);
        self.phase = None;
        self.waiting = None;
        if let Some(context) = self.spent(fx) {
            fx.emit(DriverOutput::Context(context));
        }
        fx.emit(DriverOutput::TurnEnded(if interrupted {
            TurnEnd::Interrupted
        } else {
            TurnEnd::Completed
        }));
    }

    /// Ends the turn once the session file holds `want`, its last message, reading it again
    /// shortly if it does not yet.
    fn end_when_written(&mut self, want: String, tries: u32, fx: &mut Effects) {
        self.read_text(fx);
        let written =
            want.is_empty() || self.last_text.as_deref().is_some_and(|t| t.trim() == want);
        if written || tries >= ROWS_TRIES {
            if !written {
                fx.emit(DriverOutput::Warning(
                    "Claude Code's session file did not show the turn's last message; its usage \
                     may be short"
                        .into(),
                ));
            }
            self.end_turn(fx);
        } else {
            self.wait(
                Waiting::Rows {
                    want,
                    tries: tries + 1,
                },
                ROWS_EVERY,
                fx,
            );
        }
    }

    fn fail(&mut self, detail: String, provider: Option<ProviderFailure>, fx: &mut Effects) {
        self.phase = None;
        self.waiting = None;
        self.spent(fx);
        fx.emit(DriverOutput::Failed(Failure::Error { detail, provider }));
    }

    /// A failed turn's category, or failing that what the program said and showed: a usage limit
    /// reached on a subscription is a rate limit, though its category may say nothing.
    fn classify(&self, category: Option<&str>, detail: &str) -> Option<ProviderFailure> {
        classify_claude(category, None, detail).or_else(|| {
            (looks_like_usage_limit(detail) || looks_like_usage_limit(&self.screen.text()))
                .then_some(ProviderFailure::RateLimited { retry_after: None })
        })
    }

    fn tool_output(response: &Value) -> String {
        match response {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            Value::Object(map) if map.contains_key("stdout") => {
                let mut out = map["stdout"].as_str().unwrap_or_default().to_owned();
                if let Some(err) = map
                    .get("stderr")
                    .and_then(Value::as_str)
                    .filter(|e| !e.is_empty())
                {
                    out.push('\n');
                    out.push_str(err);
                }
                out
            }
            other => other.to_string(),
        }
    }
}

/// Whether text reads like a subscription's usage limit, as Claude Code words it on screen.
pub(crate) fn looks_like_usage_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("limit")
        && (text.contains("usage") || text.contains("hit your") || text.contains("reset"))
}

impl Protocol for ClaudeTerminal {
    fn begin(&mut self, fx: &mut Effects) {
        self.wait(Waiting::Start, START_LIMIT, fx);
    }

    fn line(&mut self, _line: &str, _fx: &mut Effects) {}

    fn hook(&mut self, call: HookCall, fx: &mut Effects) {
        let body = &call.body;
        if let (None, Some(path)) = (&self.transcript, body["transcript_path"].as_str()) {
            self.transcript = Some(Transcript::new(path.into()));
        }
        match call.event.as_str() {
            "PreToolUse" => {
                self.read_text(fx);
                fx.emit(DriverOutput::ToolCall {
                    id: body["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                    name: body["tool_name"].as_str().unwrap_or_default().to_owned(),
                    input: body["tool_input"].clone(),
                });
            }
            "PostToolUse" | "PostToolUseFailure" => {
                let is_error = call.event == "PostToolUseFailure";
                let output = if is_error {
                    Self::tool_output(&body["error"])
                } else {
                    Self::tool_output(&body["tool_response"])
                };
                fx.emit(DriverOutput::ToolResult {
                    id: body["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                    output,
                    is_error,
                });
            }
            "PermissionRequest" => {
                let Some(reply) = call.reply else { return };
                self.next_ask += 1;
                let id = format!("hook-{}", self.next_ask);
                self.asked.insert(id.clone(), reply);
                fx.emit(DriverOutput::PermissionRequest(PermissionRequest {
                    id: PermissionId(id),
                    tool: body["tool_name"].as_str().unwrap_or_default().to_owned(),
                    input: body["tool_input"].clone(),
                }));
            }
            "Stop" if self.phase == Some(Phase::Turn) => {
                let want = body["last_assistant_message"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                self.end_when_written(want, 0, fx);
            }
            "StopFailure" if self.phase.is_some() => {
                let detail = body["last_assistant_message"]
                    .as_str()
                    .or(body["error_details"].as_str())
                    .unwrap_or("the turn failed")
                    .to_owned();
                let provider = self.classify(body["error"].as_str(), &detail);
                self.fail(detail, provider, fx);
            }
            "PostCompact" if self.phase == Some(Phase::Compaction) => {
                self.phase = None;
                self.waiting = None;
                // The summary's request is not in the session file, and the context it leaves is
                // known only once the next request is made.
                self.spent(fx);
                fx.emit(DriverOutput::Compacted);
            }
            _ => {}
        }
    }

    fn reported(&mut self, report: &Reported, fx: &mut Effects) {
        let Reported::Status { state, message } = report else {
            return;
        };
        match state {
            Status::Idle | Status::Done => {
                self.idle = true;
                self.maybe_ready(fx);
                // After `Stop`, the turn is already ending.
                let ending = matches!(self.waiting, Some(Waiting::Rows { .. }));
                if self.phase != Some(Phase::Turn) || ending {
                    return;
                }
                if self.interrupting {
                    self.end_turn(fx);
                } else {
                    let turn = self.turn;
                    self.wait(Waiting::Hook { turn, error: None }, HOOK_GRACE, fx);
                }
            }
            Status::Error
                if self.phase == Some(Phase::Turn)
                    && !matches!(self.waiting, Some(Waiting::Rows { .. })) =>
            {
                let turn = self.turn;
                let error = Some(message.clone().unwrap_or_else(|| "the turn failed".into()));
                self.wait(Waiting::Hook { turn, error }, HOOK_GRACE, fx);
            }
            Status::Blocked(kind) if kind == "auth" => {
                // Nobody will log in at this terminal: the login is refused or has expired.
                let detail = message
                    .clone()
                    .unwrap_or_else(|| "Claude Code asks for a login".into());
                self.fail(detail, Some(ProviderFailure::KeyRefused), fx);
            }
            Status::Blocked(kind) => fx.emit(DriverOutput::Warning(format!(
                "Claude Code is waiting on its terminal for a {kind}: {}",
                message.as_deref().unwrap_or_default()
            ))),
            Status::Working => self.idle = false,
            Status::Error | Status::Clear => {}
        }
    }

    fn wake(&mut self, fx: &mut Effects) {
        match self.waiting.take() {
            Some(Waiting::Start) if !self.ready => {
                let detail = format!(
                    "Claude Code did not show its prompt within {START_LIMIT:?}; its screen:\n{}",
                    self.screen.text()
                );
                let provider = self.classify(None, &detail);
                fx.emit(DriverOutput::Failed(Failure::Error { detail, provider }));
            }
            Some(Waiting::Rows { want, tries }) if self.phase == Some(Phase::Turn) => {
                self.end_when_written(want, tries, fx);
            }
            Some(Waiting::Hook { turn, error }) if turn == self.turn && self.phase.is_some() => {
                match error {
                    None => {
                        fx.emit(DriverOutput::Warning(
                            "Claude Code's Stop hook did not come; the turn was ended when its \
                             terminal reported it done"
                                .into(),
                        ));
                        self.end_turn(fx);
                    }
                    Some(detail) => {
                        let provider = self.classify(None, &detail);
                        self.fail(detail, provider, fx);
                    }
                }
            }
            _ => {}
        }
    }

    fn input(&mut self, input: Input, fx: &mut Effects) -> Result<(), InputError> {
        match input {
            Input::Turn(text) => {
                self.phase = Some(Phase::Turn);
                self.turn += 1;
                self.idle = false;
                // An escape inside the text could end the paste early and be read as keys.
                let text = text.replace('\x1b', "");
                let mut paste = b"\x1b[200~".to_vec();
                paste.extend_from_slice(text.as_bytes());
                paste.extend_from_slice(b"\x1b[201~");
                fx.keys.extend([
                    Key::Bytes(paste),
                    Key::Pause(ENTER_AFTER_PASTE),
                    Key::Bytes(b"\r".to_vec()),
                ]);
            }
            Input::Compact => {
                self.phase = Some(Phase::Compaction);
                fx.keys.extend([
                    Key::Bytes(b"/compact".to_vec()),
                    Key::Pause(ENTER_AFTER_COMMAND),
                    Key::Bytes(b"\r".to_vec()),
                ]);
            }
            Input::Interrupt => {
                self.interrupting = true;
                fx.keys.push(Key::Bytes(b"\x1b".to_vec()));
            }
            Input::Answer(PermissionId(id), decision) => {
                let Some(reply) = self.asked.remove(&id) else {
                    return Err(InputError::NoSuchRequest(PermissionId(id)));
                };
                let decision = match decision {
                    Decision::Allow => json!({ "behavior": "allow" }),
                    Decision::Deny { reason } => json!({ "behavior": "deny", "message": reason }),
                };
                let _ = reply.send(json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PermissionRequest",
                        "decision": decision,
                    }
                }));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Size;

    #[test]
    fn a_nearly_used_window_is_warned_about_once_at_a_turn_end() {
        let dir = tempfile::tempdir().unwrap();
        let status = dir.path().join("status.json");
        let price = crate::claude::models()[3].levels[0].1;
        let setup = Setup {
            version: "2.1.322".into(),
            warnings: Vec::new(),
            session: "s".into(),
            price,
            window: 1_000_000,
            status: status.clone(),
        };
        let mut claude = ClaudeTerminal::new(setup, Screen::blank(Size::default()));
        let turn = |claude: &mut ClaudeTerminal, seven_day: u64| {
            // The shape a Max login reported on 2026-11-12.
            let reported = json!({ "rate_limits": {
                "five_hour": { "resets_at": 1794480600, "used_percentage": 67 },
                "seven_day": { "resets_at": 1794891600, "used_percentage": seven_day },
            }});
            std::fs::write(&status, reported.to_string()).unwrap();
            let mut fx = Effects::default();
            claude.input(Input::Turn("go".into()), &mut fx).unwrap();
            let stop = HookCall {
                event: "Stop".into(),
                body: json!({ "last_assistant_message": "" }),
                reply: None,
            };
            claude.hook(stop, &mut fx);
            assert_eq!(
                fx.emit.last(),
                Some(&DriverOutput::TurnEnded(TurnEnd::Completed))
            );
            fx.emit
                .into_iter()
                .filter_map(|o| match o {
                    DriverOutput::Warning(w) => Some(w),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(turn(&mut claude, 70).is_empty());
        let said = turn(&mut claude, 92);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("seven-day") && said[0].contains("92%"),
            "{said:?}"
        );
        assert!(turn(&mut claude, 97).is_empty(), "said once");
    }

    #[test]
    fn usage_limits_are_told_from_other_errors() {
        assert!(looks_like_usage_limit(
            "You've hit your session limit · resets 3pm"
        ));
        assert!(looks_like_usage_limit(
            "Claude usage limit reached. Your limit will reset at 3pm"
        ));
        assert!(!looks_like_usage_limit(
            "API Error: 500 fake api_error. This is a server-side issue"
        ));
        assert!(!looks_like_usage_limit(
            "Please run /login · API Error: 401"
        ));
    }
}
