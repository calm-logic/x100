//! The fake harness set up with a script, a fake clock and a recording [`DaemonTools`].

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use x100_core::effort::Effort;
use x100_core::id::DriverId;
use x100_core::time::{FakeClock, Timestamp};
use x100_core::tool::ToolCall;
use x100_drivers::ToolEndpoint;
use x100_drivers::fake::{DaemonTools, FakeHarness};

use super::spec;

pub const FAKE: &str = env!("CARGO_BIN_EXE_x100-fake-harness");

pub const POSING: &str = r#"
[harness]
name = "fake-anthropic"
version = "2.0.0"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium"]
price = { input = 1.0, output = 5.0, cache_read = 0.1 }
"#;

#[derive(Default)]
pub struct Recorder {
    pub calls: Mutex<Vec<(DriverId, Option<ToolEndpoint>, ToolCall)>>,
}

#[async_trait]
impl DaemonTools for Recorder {
    async fn call(
        &self,
        driver: DriverId,
        endpoint: Option<ToolEndpoint>,
        call: ToolCall,
    ) -> Result<String, String> {
        let reply = match &call {
            ToolCall::SetPreview { .. } => Ok("previewing".to_owned()),
            // Written out, not built with `json!`: its key order depends on a serde_json feature another
            // crate in the same build may turn on.
            ToolCall::Remember { .. } => Ok(r#"{"n":7,"note":"kept"}"#.to_string()),
            _ => Err("not in this test".to_owned()),
        };
        self.calls.lock().unwrap().push((driver, endpoint, call));
        reply
    }
}

pub struct Setup {
    pub _dir: tempfile::TempDir,
    pub work: std::path::PathBuf,
    pub clock: FakeClock,
    pub harness: FakeHarness,
    pub tools: Arc<Recorder>,
}

pub fn setup(script: &str) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script.toml");
    std::fs::write(&path, format!("{POSING}\n{script}")).unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let clock = FakeClock::new(Timestamp::from_millis(1_000_000));
    let tools = Arc::new(Recorder::default());
    let harness = FakeHarness::new(
        FAKE,
        &path,
        Arc::new(clock.clone()),
        Some(Arc::clone(&tools) as Arc<dyn DaemonTools>),
    )
    .unwrap();
    Setup {
        _dir: dir,
        work,
        clock,
        harness,
        tools,
    }
}

pub fn spec_in(work: &Path) -> x100_drivers::DriverSpec {
    spec(1, work, "fake-small", Effort::Low)
}
