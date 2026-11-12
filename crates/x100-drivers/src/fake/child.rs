//! The fake harness's child process: reads its script, then plays turns as its driver gives them.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use super::script::{FailKind, Script, Step};
use super::wire::{FromFake, ToFake};

/// The fake harness's entry point: `program <script.toml>`, speaking JSON lines on stdin and
/// stdout. Exits 0 when its stdin closes.
pub fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: x100-fake-harness <script.toml>");
        std::process::exit(2);
    };
    let script = match Script::load(Path::new(&path)) {
        Ok(script) => script,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let code = runtime.block_on(async { Player::new(script).play().await });
    std::process::exit(code);
}

fn out(message: &FromFake) {
    let mut stdout = std::io::stdout().lock();
    let line = serde_json::to_string(message).expect("serializable");
    // A driver that stopped reading is being stopped; nothing useful can be done about it here.
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

/// How a run of steps ended.
enum Flow {
    Done,
    Interrupted,
    /// The script failed or stalled on purpose: do nothing more until stopped.
    Park,
}

struct Player {
    script: Script,
    used: Vec<bool>,
    vars: HashMap<String, String>,
    start_pack: String,
    input: String,
    next_id: u64,
    inbox: mpsc::UnboundedReceiver<ToFake>,
}

impl Player {
    fn new(script: Script) -> Self {
        let (tx, inbox) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ToFake>(&line) {
                    Ok(message) => {
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    Err(e) => eprintln!("not a message for the fake harness: {e}: {line}"),
                }
            }
        });
        let used = vec![false; script.turns.len()];
        Self {
            script,
            used,
            vars: HashMap::new(),
            start_pack: String::new(),
            input: String::new(),
            next_id: 0,
            inbox,
        }
    }

    async fn play(mut self) -> i32 {
        let Some(ToFake::Start {
            start_pack,
            model,
            effort,
        }) = self.inbox.recv().await
        else {
            return 0;
        };
        let offered = self
            .script
            .models
            .iter()
            .any(|m| m.model == model && m.efforts.contains(&effort));
        if !offered {
            fail(
                FailKind::Run,
                format!("the script offers no {model} at {effort:?}"),
            );
            return self.park().await;
        }
        self.start_pack = start_pack;
        out(&FromFake::Ready {
            version: self.script.harness.version.clone(),
        });
        while let Some(message) = self.inbox.recv().await {
            let flow = match message {
                ToFake::Turn { input } => self.turn(input).await,
                ToFake::Compact => self.compact().await,
                _ => Flow::Done,
            };
            if let Flow::Park = flow {
                return self.park().await;
            }
        }
        0
    }

    async fn park(mut self) -> i32 {
        while self.inbox.recv().await.is_some() {}
        0
    }

    async fn turn(&mut self, input: String) -> Flow {
        let chosen = self
            .script
            .turns
            .iter()
            .enumerate()
            .find(|(i, t)| (t.repeat || !self.used[*i]) && t.when.matches(&self.start_pack, &input))
            .map(|(i, _)| i);
        let Some(index) = chosen else {
            fail(
                FailKind::Run,
                format!("the script has no turn left for the input {input:?}"),
            );
            return Flow::Park;
        };
        self.used[index] = true;
        self.input = input;
        let steps = self.script.turns[index].steps.clone();
        let flow = self.steps(&steps).await;
        if !matches!(flow, Flow::Park) {
            out(&FromFake::TurnEnded {
                interrupted: matches!(flow, Flow::Interrupted),
            });
        }
        flow
    }

    async fn compact(&mut self) -> Flow {
        let steps = self.script.compaction.steps.clone();
        let flow = self.steps(&steps).await;
        if matches!(flow, Flow::Park) {
            return flow;
        }
        if let Some(used) = self.script.compaction.context_after {
            out(&FromFake::Context { used });
        }
        out(&FromFake::Compacted);
        Flow::Done
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    async fn steps(&mut self, steps: &[Step]) -> Flow {
        for step in steps {
            match self.step(step).await {
                Flow::Done => {}
                other => return other,
            }
        }
        Flow::Done
    }

    async fn step(&mut self, step: &Step) -> Flow {
        match step {
            Step::Text(text) => out(&FromFake::Text {
                text: self.fill(text),
            }),
            Step::Call { call, save } => {
                let input = self.fill_json(call);
                let name = input
                    .as_object()
                    .and_then(|o| o.keys().next().cloned())
                    .unwrap_or_else(|| "call".into());
                let id = self.id("call");
                out(&FromFake::ToolCall {
                    id: id.clone(),
                    name,
                    input,
                    daemon: true,
                });
                let reply = self
                    .reply(|m| matches!(m, ToFake::DaemonResult { id: got, .. } if *got == id))
                    .await;
                let Some(ToFake::DaemonResult { text, .. }) = reply else {
                    return Flow::Interrupted;
                };
                if let Some(name) = save {
                    self.vars.insert(name.clone(), text);
                }
            }
            Step::Write { path, content } => {
                let path = self.fill(path);
                let content = self.fill(content);
                let id = self.id("write");
                out(&FromFake::ToolCall {
                    id: id.clone(),
                    name: "write".into(),
                    input: serde_json::json!({ "path": path }),
                    daemon: false,
                });
                let written = write_file(Path::new(&path), &content);
                out(&FromFake::ToolResult {
                    id,
                    output: written.clone().unwrap_or_else(|e| e),
                    is_error: written.is_err(),
                });
            }
            Step::Run(command) => return self.run(self.fill(command)).await,
            Step::Ask { tool, input } => {
                return self.ask(tool.clone(), self.fill_json(input)).await;
            }
            Step::Usage(usage) => out(&FromFake::Usage(*usage)),
            Step::Context(used) => out(&FromFake::Context { used: *used }),
            Step::Sleep(duration) => {
                tokio::select! {
                    _ = tokio::time::sleep(*duration) => {}
                    _ = self.interrupted() => return Flow::Interrupted,
                }
            }
            Step::Crash(code) => std::process::exit(*code),
            // Silent from here on; it still exits when its stdin closes, as a hung harness's
            // process would when killed.
            Step::Stall => return Flow::Park,
            Step::Fail { kind, detail } => {
                fail(*kind, self.fill(detail));
                return Flow::Park;
            }
        }
        Flow::Done
    }

    async fn run(&mut self, command: String) -> Flow {
        let id = self.id("run");
        out(&FromFake::ToolCall {
            id: id.clone(),
            name: "run".into(),
            input: serde_json::json!({ "command": command }),
            daemon: false,
        });
        // Left in the fake's own process group, so stopping the driver stops it too.
        let child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(e) => {
                out(&FromFake::ToolResult {
                    id,
                    output: e.to_string(),
                    is_error: true,
                });
                return Flow::Done;
            }
        };
        tokio::select! {
            result = child.wait_with_output() => {
                let (output, is_error) = match result {
                    Ok(o) => {
                        let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                        text.push_str(&String::from_utf8_lossy(&o.stderr));
                        (text, !o.status.success())
                    }
                    Err(e) => (e.to_string(), true),
                };
                out(&FromFake::ToolResult { id, output, is_error });
                Flow::Done
            }
            _ = self.interrupted() => {
                out(&FromFake::ToolResult { id, output: "interrupted".into(), is_error: true });
                Flow::Interrupted
            }
        }
    }

    async fn ask(&mut self, tool: String, input: Value) -> Flow {
        let call_id = self.id("tool");
        let ask_id = self.id("ask");
        out(&FromFake::ToolCall {
            id: call_id.clone(),
            name: tool.clone(),
            input: input.clone(),
            daemon: false,
        });
        out(&FromFake::Permission {
            id: ask_id.clone(),
            tool,
            input,
        });
        let answer = self
            .reply(|m| matches!(m, ToFake::PermissionAnswer { id, .. } if *id == ask_id))
            .await;
        let Some(ToFake::PermissionAnswer { allow, reason, .. }) = answer else {
            return Flow::Interrupted;
        };
        out(&FromFake::ToolResult {
            id: call_id,
            output: if allow {
                "allowed".into()
            } else {
                format!("denied: {reason}")
            },
            is_error: !allow,
        });
        Flow::Done
    }

    /// The next message `wanted` accepts, or `None` if the turn is interrupted or stdin closes.
    async fn reply(&mut self, wanted: impl Fn(&ToFake) -> bool) -> Option<ToFake> {
        while let Some(message) = self.inbox.recv().await {
            if matches!(message, ToFake::Interrupt) {
                return None;
            }
            if wanted(&message) {
                return Some(message);
            }
        }
        None
    }

    /// Resolves when an interrupt arrives or stdin closes; other messages mid-step are dropped,
    /// since the driver sends nothing else while a step runs.
    async fn interrupted(&mut self) {
        while let Some(message) = self.inbox.recv().await {
            if matches!(message, ToFake::Interrupt) {
                return;
            }
        }
    }

    /// Replaces `{{name}}` with a saved value, `{{name.field}}` with a field of a saved JSON
    /// object, and `{{input}}` with the turn's input. Anything else is left as written.
    fn fill(&self, text: &str) -> String {
        let mut result = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            let Some(len) = rest[start..].find("}}") else {
                break;
            };
            let name = &rest[start + 2..start + len];
            result.push_str(&rest[..start]);
            match self.lookup(name.trim()) {
                Some(value) => result.push_str(&value),
                None => result.push_str(&rest[start..start + len + 2]),
            }
            rest = &rest[start + len + 2..];
        }
        result.push_str(rest);
        result
    }

    fn lookup(&self, name: &str) -> Option<String> {
        if name == "input" {
            return Some(self.input.clone());
        }
        if let Some(value) = self.vars.get(name) {
            return Some(value.clone());
        }
        let (var, field) = name.split_once('.')?;
        let json: Value = serde_json::from_str(self.vars.get(var)?).ok()?;
        match json.get(field)? {
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }

    fn fill_json(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.fill(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.fill_json(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.fill_json(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

fn fail(kind: FailKind, detail: String) {
    out(&FromFake::Error { detail, kind });
}

fn write_file(path: &Path, content: &str) -> Result<String, String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(format!(
        "wrote {} bytes to {}",
        content.len(),
        path.display()
    ))
}
