//! The daemon's tools for agents arrive with M6. Until then a fake-harness driver that calls one
//! gets an error that says so.

use async_trait::async_trait;
use x100_core::id::DriverId;
use x100_core::tool::ToolCall;
use x100_drivers::ToolEndpoint;
use x100_drivers::fake::DaemonTools;

#[derive(Debug, Default, Clone, Copy)]
pub struct NoTools;

pub const NOT_YET: &str = "the daemon's tools are not available yet: they arrive with M6";

#[async_trait]
impl DaemonTools for NoTools {
    async fn call(
        &self,
        _driver: DriverId,
        _endpoint: Option<ToolEndpoint>,
        _call: ToolCall,
    ) -> Result<String, String> {
        Err(NOT_YET.to_owned())
    }
}
