//! What background work hands back to the interface loop.

use x100_client::ClientError;
use x100_core::event::{Envelope, ProjectEvent};
use x100_core::file::FileRef;
use x100_core::id::{AgentId, DriverId, ProjectId};
use x100_core::terminal::Chunk;
use x100_core::transcript::Entry;
use x100_proto::{Memories, Skills};

use crate::logfeed::Read;

pub enum Msg {
    Entry {
        token: u64,
        entry: Entry,
    },
    /// A terminal driver's subscription was taken.
    TerminalOpened {
        token: u64,
        driver: DriverId,
    },
    Chunk {
        token: u64,
        driver: DriverId,
        chunk: Chunk,
    },
    /// The daemon will not show this driver's terminal.
    TerminalRefused {
        token: u64,
        driver: DriverId,
        no_terminal: bool,
        error: String,
    },
    Log {
        project: ProjectId,
        read: Read,
        result: Result<Vec<Envelope<ProjectEvent>>, ClientError>,
    },
    Memories {
        agent: AgentId,
        result: Result<Memories, ClientError>,
    },
    Skills {
        agent: AgentId,
        result: Result<Skills, ClientError>,
    },
    /// A project was made and its first message sent.
    Started(Result<ProjectId, ClientError>),
    Uploaded(Result<FileRef, ClientError>),
    Notice(String),
}
