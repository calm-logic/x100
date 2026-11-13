//! The scripted fake harness.
//!
//! A real child process (the `x100-fake-harness` binary, or any binary whose `main` calls
//! [`child_main`]) plays a [`script::Script`], so starting, streaming, crashing, stalling and
//! stopping are real process behaviour. Calls to the daemon's tools go to a [`DaemonTools`] the
//! test or the daemon supplies, and their results go back into the script.

mod child;
pub mod script;
mod wire;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use x100_core::driver::{ContextFill, Failure};
use x100_core::harness::{HarnessKind, ModelOffer, ProviderFailure, Support};
use x100_core::id::DriverId;
use x100_core::ledger::CostBasis;
use x100_core::money::{Money, Price, Usage};
use x100_core::time::Clock;
use x100_core::tool::ToolCall;

pub use child::main as child_main;
use script::{FailKind, Script, ScriptError, UsageSpec};
use wire::{FromFake, ToFake};

use crate::handle::{DriverHandle, InputError};
use crate::harness::{CommandLine, DriverSpec, Harness, StartError, ToolEndpoint, check_offered};
use crate::output::{Decision, DriverOutput, PermissionId, PermissionRequest, Spent, TurnEnd};
use crate::protocol::{Effects, Input, Protocol, Ready};
use crate::supervisor::{DaemonCall, Launch};

/// A daemon tool call's result to come: `Ok` is what the driver sees, `Err` an error it sees.
pub type Pending = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

/// The daemon's tools as the fake harness reaches them. The real harnesses reach the same tools
/// over MCP at [`DriverSpec::tools`].
pub trait DaemonTools: Send + Sync {
    /// Takes one call from `driver`, which reached the tools at `endpoint`, the moment the driver
    /// makes it. Not async, so whatever the tools must note of the call at once, such as that it
    /// is in flight, is noted before the driver's next move, its death included.
    fn call(
        self: Arc<Self>,
        driver: DriverId,
        endpoint: Option<ToolEndpoint>,
        call: ToolCall,
    ) -> Pending;
}

pub struct FakeHarness {
    program: PathBuf,
    script_path: PathBuf,
    script: Script,
    clock: Arc<dyn Clock>,
    tools: Option<Arc<dyn DaemonTools>>,
}

impl FakeHarness {
    /// `program` is the fake harness binary; `script` is read now, so a broken script is refused
    /// here rather than in every driver.
    pub fn new(
        program: impl Into<PathBuf>,
        script: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
        tools: Option<Arc<dyn DaemonTools>>,
    ) -> Result<Self, ScriptError> {
        let script_path = script.into();
        let script = Script::load(&script_path)?;
        Ok(Self {
            program: program.into(),
            script_path,
            script,
            clock,
            tools,
        })
    }
}

#[async_trait]
impl Harness for FakeHarness {
    fn kind(&self) -> HarnessKind {
        self.script.kind()
    }

    fn models(&self) -> Vec<ModelOffer> {
        self.script.offers()
    }

    fn supports(&self) -> Support {
        self.script.support()
    }

    async fn start(&self, spec: DriverSpec) -> Result<DriverHandle, StartError> {
        let models = self.models();
        check_offered(self.kind(), &models, &spec, self.supports())?;
        let offer = models
            .into_iter()
            .find(|m| m.model == spec.model)
            .expect("checked as offered");
        let price = offer
            .levels
            .iter()
            .find(|(e, _)| *e == spec.effort)
            .map(|(_, p)| *p)
            .expect("checked as offered");
        let line = spec.wrap.apply(CommandLine {
            program: self.program.clone(),
            args: vec![self.script_path.clone().into_os_string()],
        });
        let mut env: Vec<(String, String)> = std::env::var("PATH")
            .map(|path| vec![("PATH".to_owned(), path)])
            .unwrap_or_default();
        env.extend(spec.env.iter().cloned());
        let daemon_call = self.tools.clone().map(|tools| {
            let driver = spec.driver;
            let endpoint = spec.tools.clone();
            let call: DaemonCall =
                Arc::new(move |call| Arc::clone(&tools).call(driver, endpoint.clone(), call));
            call
        });
        let protocol = FakeProtocol {
            start: ToFake::Start {
                start_pack: spec.start_pack.clone(),
                model: spec.model.0.clone(),
                effort: spec.effort,
                tools: spec.tools.clone().map(|t| (t.url, t.token)),
            },
            window: offer.context_window,
            price,
        };
        let launch = Launch {
            support: self.supports(),
            stall: spec.stall,
            permissions: spec.permissions,
            clock: Arc::clone(&self.clock),
            daemon_call,
            cleanup: None,
        };
        crate::wire::launch_piped(
            launch,
            &line,
            &env,
            &spec.dir,
            &spec.on_spawn,
            Box::new(protocol),
        )
        .await
    }
}

struct FakeProtocol {
    start: ToFake,
    window: u64,
    price: Price,
}

fn send(fx: &mut Effects, message: &ToFake) {
    fx.send(serde_json::to_string(message).expect("serializable"));
}

impl Protocol for FakeProtocol {
    fn begin(&mut self, fx: &mut Effects) {
        send(fx, &self.start);
    }

    fn line(&mut self, line: &str, fx: &mut Effects) {
        let Ok(message) = serde_json::from_str::<FromFake>(line) else {
            fx.emit(DriverOutput::Failed(Failure::StreamBroke));
            return;
        };
        let out = match message {
            FromFake::Ready { version } => {
                fx.started = Some(Ok(Ready {
                    version,
                    warnings: Vec::new(),
                    session: None,
                }));
                return;
            }
            FromFake::Text { text } => DriverOutput::Text(text),
            FromFake::ToolCall {
                id,
                name,
                input,
                daemon,
            } => {
                if daemon {
                    match serde_json::from_value::<ToolCall>(input.clone()) {
                        Ok(call) => fx.daemon_calls.push((id.clone(), call)),
                        Err(e) => {
                            fx.emit(DriverOutput::Failed(Failure::Error {
                                detail: format!(
                                    "the script made a call that is not a daemon tool call: {e}"
                                ),
                                provider: None,
                            }));
                            return;
                        }
                    }
                }
                DriverOutput::ToolCall { id, name, input }
            }
            FromFake::ToolResult {
                id,
                output,
                is_error,
            } => DriverOutput::ToolResult {
                id,
                output,
                is_error,
            },
            FromFake::Permission { id, tool, input } => {
                DriverOutput::PermissionRequest(PermissionRequest {
                    id: PermissionId(id),
                    tool,
                    input,
                })
            }
            FromFake::Usage(usage) => DriverOutput::Spent(self.spent(usage)),
            FromFake::Context { used } => DriverOutput::Context(ContextFill {
                used,
                window: self.window,
            }),
            FromFake::TurnEnded { interrupted } => DriverOutput::TurnEnded(if interrupted {
                TurnEnd::Interrupted
            } else {
                TurnEnd::Completed
            }),
            FromFake::Compacted => DriverOutput::Compacted,
            FromFake::Error { detail, kind } => {
                let provider = match kind {
                    FailKind::Outage => Some(ProviderFailure::Outage {
                        detail: detail.clone(),
                    }),
                    FailKind::RateLimited => {
                        Some(ProviderFailure::RateLimited { retry_after: None })
                    }
                    FailKind::KeyRefused => Some(ProviderFailure::KeyRefused),
                    FailKind::Run => None,
                };
                DriverOutput::Failed(Failure::Error { detail, provider })
            }
        };
        fx.emit(out);
    }

    fn input(&mut self, input: Input, fx: &mut Effects) -> Result<(), InputError> {
        let message = match input {
            Input::Turn(input) => ToFake::Turn { input },
            Input::Compact => ToFake::Compact,
            Input::Interrupt => ToFake::Interrupt,
            Input::Answer(id, decision) => {
                let (allow, reason) = match decision {
                    Decision::Allow => (true, String::new()),
                    Decision::Deny { reason } => (false, reason),
                };
                ToFake::PermissionAnswer {
                    id: id.0,
                    allow,
                    reason,
                }
            }
        };
        send(fx, &message);
        Ok(())
    }

    /// Only passed on: the child reports the result once it has read it, as a real harness does.
    /// A child that dies first never saw it, so the call stays open and the next driver is told
    /// it was in flight.
    fn daemon_reply(&mut self, id: String, reply: Result<String, String>, fx: &mut Effects) {
        let (ok, text) = match reply {
            Ok(text) => (true, text),
            Err(text) => (false, text),
        };
        send(fx, &ToFake::DaemonResult { id, ok, text });
    }
}

impl FakeProtocol {
    fn spent(&self, spec: UsageSpec) -> Spent {
        let usage = Usage {
            input: spec.input,
            output: spec.output,
            cache_read: spec.cache_read,
            cache_write_short: spec.cache_write_short,
            cache_write_long: spec.cache_write_long,
        };
        match spec.cost_micros {
            Some(micros) => Spent {
                usage,
                cost: Money::from_micros(micros),
                basis: CostBasis::Reported,
            },
            None => Spent {
                usage,
                cost: self.price.cost(&usage),
                basis: CostBasis::Listed,
            },
        }
    }
}
