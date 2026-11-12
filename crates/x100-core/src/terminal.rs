//! A terminal driver's terminal, recorded: what its program wrote and every resize, in order. The
//! store keeps it and the client protocol carries it, so the type lives here, where both may depend
//! on it.
//!
//! Both programs draw on the alternate screen with absolute positions, so a replay is only right at
//! the size the program had when it drew. A client therefore replays the chunks from the first into
//! its own terminal emulator, resizing where a chunk says so.

use serde::{Deserialize, Serialize};

use crate::bytes::Bytes;
use crate::time::Timestamp;

/// A chunk's place in one driver's terminal. Counts from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TermPos(pub u64);

/// One piece of a driver's terminal. The first chunk of every terminal is a `Resize`: its starting
/// size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub at: Timestamp,
    pub what: ChunkKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChunkKind {
    /// Bytes the program wrote. They are not text and may split a character or an escape sequence.
    Output {
        bytes: Bytes,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
}
