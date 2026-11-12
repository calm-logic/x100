use tokio::sync::mpsc;
use x100_core::terminal::{Chunk, TermPos};
use x100_core::transcript::{Entry, Pos};

use crate::Cmd;

/// An agent's transcript as it is written. Ends when the client closes or the agent is gone.
pub type TranscriptStream = Follow<Pos, Entry>;

/// A terminal driver's terminal as its program writes it. Ends when the client closes or the
/// driver has no terminal.
pub type TerminalStream = Follow<TermPos, Chunk>;

/// Something followed from a position on: each item once, in order, across reconnects.
pub struct Follow<P, T> {
    id: u64,
    items: mpsc::UnboundedReceiver<(P, T)>,
    cmds: mpsc::UnboundedSender<Cmd>,
}

impl<P, T> Follow<P, T> {
    pub(crate) fn new(
        id: u64,
        items: mpsc::UnboundedReceiver<(P, T)>,
        cmds: mpsc::UnboundedSender<Cmd>,
    ) -> Self {
        Self { id, items, cmds }
    }

    pub async fn next(&mut self) -> Option<(P, T)> {
        self.items.recv().await
    }
}

impl<P, T> Drop for Follow<P, T> {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Unfollow(self.id));
    }
}
