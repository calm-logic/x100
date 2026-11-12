use tokio::sync::mpsc;
use x100_core::transcript::{Entry, Pos};

use crate::Cmd;

/// An agent's transcript as it is written. Ends when the client closes.
pub struct TranscriptStream {
    id: u64,
    entries: mpsc::UnboundedReceiver<(Pos, Entry)>,
    cmds: mpsc::UnboundedSender<Cmd>,
}

impl TranscriptStream {
    pub(crate) fn new(
        id: u64,
        entries: mpsc::UnboundedReceiver<(Pos, Entry)>,
        cmds: mpsc::UnboundedSender<Cmd>,
    ) -> Self {
        Self { id, entries, cmds }
    }

    pub async fn next(&mut self) -> Option<(Pos, Entry)> {
        self.entries.recv().await
    }
}

impl Drop for TranscriptStream {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Unfollow(self.id));
    }
}
