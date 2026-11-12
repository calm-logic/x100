//! Following a terminal driver: its outputs, with the terminal chunks kept aside as the daemon
//! would record them.

use std::path::{Path, PathBuf};

use x100_core::terminal::{Chunk, ChunkKind};
use x100_drivers::terminal::Emulator;
use x100_drivers::{DriverHandle, DriverOutput};

use super::{PATIENCE, next};

pub struct Watched {
    pub handle: DriverHandle,
    pub chunks: Vec<Chunk>,
}

impl Watched {
    pub fn new(handle: DriverHandle) -> Self {
        Self {
            handle,
            chunks: Vec::new(),
        }
    }

    /// The next output that is not a terminal chunk.
    pub async fn next(&mut self) -> Option<DriverOutput> {
        loop {
            match next(&mut self.handle).await? {
                DriverOutput::Terminal(chunk) => self.chunks.push(chunk),
                other => return Some(other),
            }
        }
    }

    pub async fn until_end(&mut self) -> Vec<DriverOutput> {
        let mut seen = Vec::new();
        while let Some(out) = self.next().await {
            let end = matches!(
                out,
                DriverOutput::TurnEnded(_) | DriverOutput::Compacted | DriverOutput::Failed(_)
            );
            seen.push(out);
            if end {
                return seen;
            }
        }
        seen
    }

    pub async fn until(&mut self, wanted: impl Fn(&DriverOutput) -> bool) -> Vec<DriverOutput> {
        let mut seen = Vec::new();
        loop {
            let out = self.next().await.unwrap_or_else(|| {
                panic!("the stream ended before the output wanted; saw {seen:#?}")
            });
            let done = wanted(&out);
            seen.push(out);
            if done {
                return seen;
            }
        }
    }

    /// Reads chunks until the recorded screen shows `text`. Fails on any other output.
    pub async fn until_screen_shows(&mut self, text: &str) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while !self.replay().contains(text) {
            let out = tokio::time::timeout_at(deadline, self.handle.next())
                .await
                .unwrap_or_else(|_| panic!("the screen never showed {text:?}:\n{}", self.replay()))
                .expect("the stream ended");
            match out {
                DriverOutput::Terminal(chunk) => self.chunks.push(chunk),
                other => panic!("expected only terminal output while waiting, got {other:?}"),
            }
        }
    }

    /// Takes in every chunk already emitted.
    pub fn drain(&mut self) -> Vec<DriverOutput> {
        let mut other = Vec::new();
        while let Some(out) = self.handle.try_next() {
            match out {
                DriverOutput::Terminal(chunk) => self.chunks.push(chunk),
                out => other.push(out),
            }
        }
        other
    }

    /// The screen the recording so far replays to.
    pub fn replay(&self) -> String {
        Emulator::replay(&self.chunks)
            .expect("a recording starts with its size")
            .text()
    }

    /// Asserts the live screen is the one the recording replays to.
    pub async fn assert_replay_matches(&mut self) {
        let (at, live) = self
            .handle
            .control()
            .screen()
            .await
            .expect("a terminal driver has a screen");
        let other = self.drain();
        assert!(other.is_empty(), "unexpected output: {other:#?}");
        assert!(
            matches!(self.chunks[0].what, ChunkKind::Resize { .. }),
            "the recording starts with its size"
        );
        let upto = usize::try_from(at.0).unwrap();
        let replayed = Emulator::replay(&self.chunks[..upto]).unwrap().text();
        assert_eq!(replayed, live, "the recording replays to the live screen");
        assert!(!live.trim().is_empty(), "the screen shows something");
    }
}

/// Every file under `dir`, relative to it, for checking what a driver wrote into a login.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
            } else {
                found.push(path.strip_prefix(dir).unwrap().to_owned());
            }
        }
    }
    found.sort();
    found
}
