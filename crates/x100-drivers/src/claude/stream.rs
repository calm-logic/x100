//! Claude Code's stream-json protocol: `SDKMessage` lines out, user messages and control requests
//! in. The shapes are those of the Agent SDK's `sdk.d.ts` at 2.1.322, as seen against the fake
//! provider.

use std::collections::HashMap;

use serde_json::{Value, json};
use x100_core::driver::{ContextFill, Failure};
use x100_core::ledger::CostBasis;
use x100_core::money::{Money, Usage};

use super::{TOOLS_SERVER, VERSIONS};
use crate::failure::classify_claude;
use crate::handle::InputError;
use crate::output::{Decision, DriverOutput, PermissionId, PermissionRequest, Spent, TurnEnd};
use crate::protocol::{Effects, Input, Protocol, Ready};

const INIT: &str = "x100-init";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Turn,
    Compaction,
}

pub(crate) struct ClaudeStream {
    start_pack: String,
    expects_tools: bool,
    phase: Option<Phase>,
    interrupting: bool,
    /// The end of a turn or compaction, held back until the context fill it asked for arrives.
    held_end: Option<DriverOutput>,
    context_request: Option<String>,
    next_request: u64,
    /// Each pending permission request's tool input, which an `allow` must send back.
    asked: HashMap<String, Value>,
    /// The error category of the turn's last assistant message, for classifying its failure.
    error_category: Option<String>,
    /// Running totals from the last `result`: Claude Code reports cost and usage since it started.
    cost_so_far: f64,
    usage_so_far: Usage,
    mcp_checked: bool,
}

impl ClaudeStream {
    pub(crate) fn new(start_pack: String, expects_tools: bool) -> Self {
        Self {
            start_pack,
            expects_tools,
            phase: None,
            interrupting: false,
            held_end: None,
            context_request: None,
            next_request: 0,
            asked: HashMap::new(),
            error_category: None,
            cost_so_far: 0.0,
            usage_so_far: Usage::default(),
            mcp_checked: false,
        }
    }

    fn request_id(&mut self, what: &str) -> String {
        self.next_request += 1;
        format!("x100-{what}-{}", self.next_request)
    }

    fn user(text: &str) -> Value {
        json!({
            "type": "user",
            "message": { "role": "user", "content": text },
            "parent_tool_use_id": null,
            "session_id": "",
        })
    }

    fn on_control_response(&mut self, response: &Value, fx: &mut Effects) {
        let id = response["request_id"].as_str().unwrap_or_default();
        let ok = response["subtype"] == "success";
        let body = &response["response"];
        if id == INIT {
            fx.started = Some(if ok {
                let version = body["claude_code_version"].as_str().unwrap_or("unknown");
                VERSIONS.check(version).map(|warning| Ready {
                    version: version.to_owned(),
                    warnings: warning.into_iter().collect(),
                    session: None,
                })
            } else {
                Err(format!("initialize failed: {}", response["error"]))
            });
            return;
        }
        if self.context_request.as_deref() == Some(id) {
            self.context_request = None;
            let used = body["totalTokens"].as_u64();
            let window = body["maxTokens"].as_u64();
            if let (true, Some(used), Some(window)) = (ok, used, window) {
                fx.emit(DriverOutput::Context(ContextFill { used, window }));
            }
            if let Some(end) = self.held_end.take() {
                fx.emit(end);
            }
        }
    }

    fn on_control_request(&mut self, message: &Value, fx: &mut Effects) {
        let id = message["request_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let request = &message["request"];
        if request["subtype"] == "can_use_tool" {
            let input = request["input"].clone();
            self.asked.insert(id.clone(), input.clone());
            fx.emit(DriverOutput::PermissionRequest(PermissionRequest {
                id: PermissionId(id),
                tool: request["tool_name"].as_str().unwrap_or_default().to_owned(),
                input,
            }));
            return;
        }
        // Hooks, elicitation and dialogs are not used; answering keeps the turn from waiting.
        fx.send_json(&json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": id,
                "error": format!("x100 does not handle {}", request["subtype"]),
            },
        }));
    }

    fn on_assistant(&mut self, message: &Value, fx: &mut Effects) {
        if let Some(category) = message["error"].as_str() {
            self.error_category = Some(category.to_owned());
        }
        for block in message["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
        {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default();
                    if !text.is_empty() {
                        fx.emit(DriverOutput::Text(text.to_owned()));
                    }
                }
                Some("tool_use") => fx.emit(DriverOutput::ToolCall {
                    id: block["id"].as_str().unwrap_or_default().to_owned(),
                    name: block["name"].as_str().unwrap_or_default().to_owned(),
                    input: block["input"].clone(),
                }),
                _ => {}
            }
        }
    }

    fn on_user(message: &Value, fx: &mut Effects) {
        for block in message["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if block["type"] != "tool_result" {
                continue;
            }
            let output = match &block["content"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            fx.emit(DriverOutput::ToolResult {
                id: block["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                output,
                is_error: block["is_error"].as_bool().unwrap_or(false),
            });
        }
    }

    fn on_init(&mut self, message: &Value, fx: &mut Effects) {
        if !self.expects_tools || self.mcp_checked {
            return;
        }
        self.mcp_checked = true;
        let connected = message["mcp_servers"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|s| s["name"] == TOOLS_SERVER && s["status"] == "connected");
        if !connected {
            fx.emit(DriverOutput::Warning(format!(
                "Claude Code could not reach the daemon's tools: {}",
                message["mcp_servers"]
            )));
        }
    }

    /// Cost and tokens since the last `result`, from the running totals Claude Code reports.
    fn spent(&mut self, result: &Value) -> Spent {
        let total_cost = result["total_cost_usd"]
            .as_f64()
            .unwrap_or(self.cost_so_far);
        let cost = ((total_cost - self.cost_so_far) * 1_000_000.0)
            .round()
            .max(0.0) as u64;
        self.cost_so_far = total_cost;
        let mut total = Usage::default();
        let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
        for model in result["modelUsage"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
        {
            let long = n(model, "cacheCreationEphemeral1hInputTokens");
            total += Usage {
                input: n(model, "inputTokens"),
                output: n(model, "outputTokens"),
                cache_read: n(model, "cacheReadInputTokens"),
                cache_write_short: n(model, "cacheCreationInputTokens").saturating_sub(long),
                cache_write_long: long,
            };
        }
        let before = self.usage_so_far;
        self.usage_so_far = total;
        Spent {
            usage: Usage {
                input: total.input.saturating_sub(before.input),
                output: total.output.saturating_sub(before.output),
                cache_read: total.cache_read.saturating_sub(before.cache_read),
                cache_write_short: total
                    .cache_write_short
                    .saturating_sub(before.cache_write_short),
                cache_write_long: total
                    .cache_write_long
                    .saturating_sub(before.cache_write_long),
            },
            cost: Money::from_micros(cost),
            basis: CostBasis::Reported,
        }
    }

    fn on_result(&mut self, result: &Value, fx: &mut Effects) {
        let phase = self.phase.take();
        let interrupted = std::mem::take(&mut self.interrupting);
        let category = self.error_category.take();
        let spent = self.spent(result);
        let is_error = result["is_error"].as_bool().unwrap_or(false);
        if is_error && !interrupted {
            let detail = result["result"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| result["errors"].to_string());
            let status = result["api_error_status"]
                .as_u64()
                .and_then(|s| u16::try_from(s).ok());
            let provider = if category.is_some() || result["terminal_reason"] == "api_error" {
                classify_claude(category.as_deref(), status, &detail)
            } else {
                None
            };
            fx.emit(DriverOutput::Spent(spent));
            fx.emit(DriverOutput::Failed(Failure::Error { detail, provider }));
            return;
        }
        fx.emit(DriverOutput::Spent(spent));
        let end = match (phase, interrupted) {
            (Some(Phase::Compaction), _) => DriverOutput::Compacted,
            (_, true) => DriverOutput::TurnEnded(TurnEnd::Interrupted),
            _ => DriverOutput::TurnEnded(TurnEnd::Completed),
        };
        // Ask how full the context is now, and end the turn when the answer arrives.
        let id = self.request_id("context");
        fx.send_json(&json!({
            "type": "control_request",
            "request_id": id,
            "request": { "subtype": "get_context_usage", "detail": "summary" },
        }));
        self.context_request = Some(id);
        self.held_end = Some(end);
    }
}

impl Protocol for ClaudeStream {
    fn begin(&mut self, fx: &mut Effects) {
        fx.send_json(&json!({
            "type": "control_request",
            "request_id": INIT,
            "request": { "subtype": "initialize", "appendSystemPrompt": self.start_pack },
        }));
    }

    fn line(&mut self, line: &str, fx: &mut Effects) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            fx.emit(DriverOutput::Failed(Failure::StreamBroke));
            return;
        };
        match message["type"].as_str() {
            Some("control_response") => self.on_control_response(&message["response"], fx),
            Some("control_request") => self.on_control_request(&message, fx),
            Some("assistant") => self.on_assistant(&message, fx),
            Some("user") => Self::on_user(&message, fx),
            Some("system") if message["subtype"] == "init" => self.on_init(&message, fx),
            Some("result") => self.on_result(&message, fx),
            _ => {}
        }
    }

    fn input(&mut self, input: Input, fx: &mut Effects) -> Result<(), InputError> {
        match input {
            Input::Turn(text) => {
                self.phase = Some(Phase::Turn);
                fx.send_json(&Self::user(&text));
            }
            Input::Compact => {
                self.phase = Some(Phase::Compaction);
                fx.send_json(&Self::user("/compact"));
            }
            Input::Interrupt => {
                self.interrupting = true;
                let id = self.request_id("interrupt");
                fx.send_json(&json!({
                    "type": "control_request",
                    "request_id": id,
                    "request": { "subtype": "interrupt" },
                }));
            }
            Input::Answer(PermissionId(id), decision) => {
                let input = self.asked.remove(&id).unwrap_or(Value::Null);
                let response = match decision {
                    Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
                    Decision::Deny { reason } => json!({ "behavior": "deny", "message": reason }),
                };
                fx.send_json(&json!({
                    "type": "control_response",
                    "response": { "subtype": "success", "request_id": id, "response": response },
                }));
            }
        }
        Ok(())
    }
}
