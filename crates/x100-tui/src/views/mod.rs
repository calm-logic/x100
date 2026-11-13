//! One module per tab. Each builds a [`Page`](crate::page::Page) from the state the client holds.

pub mod agent;
pub mod budget;
pub mod chat;
pub mod files;
pub mod held;
pub mod integrations;
pub mod log;
pub mod preview;
pub mod settings;
pub mod specs;
pub mod tasks;
pub mod team;

use x100_core::fold::{DaemonState, ProjectState};

/// What a view reads: the project and the daemon as the client holds them, and the width to wrap to.
pub struct Ctx<'a> {
    pub p: &'a ProjectState,
    pub d: &'a DaemonState,
    pub width: usize,
    pub colours: crate::look::Colours,
}

pub fn role(p: &ProjectState, id: x100_core::id::AgentId) -> String {
    p.agent(id)
        .map_or_else(|| "An agent".into(), |a| a.role.clone())
}
