//! Reading Claude Code's session file as it grows. Each model request's assistant message is
//! written there with its tokens, so it is where a terminal driver's text and usage come from.
//! Tool calls and their results come from hooks instead, which fire as they happen.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde_json::Value;
use x100_core::money::Usage;

pub(super) struct Transcript {
    path: PathBuf,
    offset: u64,
    /// The end of a line Claude Code had not finished writing when the file was last read.
    partial: Vec<u8>,
    /// Each request since the last turn end, by message id, with its tokens as last written.
    requests: Vec<(String, Usage)>,
}

/// What the new rows of the file held.
#[derive(Debug, Default)]
pub(super) struct Rows {
    pub texts: Vec<String>,
}

impl Transcript {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
            requests: Vec::new(),
        }
    }

    /// Reads the rows written since the last read.
    pub fn read(&mut self) -> Rows {
        let mut rows = Rows::default();
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return rows;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return rows;
        }
        let mut added = Vec::new();
        if file.read_to_end(&mut added).is_err() {
            return rows;
        }
        self.offset += added.len() as u64;
        self.partial.extend_from_slice(&added);
        let Some(end) = self.partial.iter().rposition(|b| *b == b'\n') else {
            return rows;
        };
        let complete: Vec<u8> = self.partial.drain(..=end).collect();
        for line in complete.split(|b| *b == b'\n') {
            if let Ok(row) = serde_json::from_slice::<Value>(line) {
                self.row(&row, &mut rows);
            }
        }
        rows
    }

    fn row(&mut self, row: &Value, rows: &mut Rows) {
        // Subagents' rows and Claude Code's own notes are not the agent's words.
        if row["type"] != "assistant" || row["isSidechain"] == true || row["isMeta"] == true {
            return;
        }
        let message = &row["message"];
        if let Some(id) = message["id"].as_str() {
            let usage = usage(&message["usage"]);
            match self.requests.iter_mut().find(|(seen, _)| seen == id) {
                Some((_, last)) => *last = usage,
                None => self.requests.push((id.to_owned(), usage)),
            }
        }
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] == "text"
                && let Some(text) = block["text"].as_str().filter(|t| !t.is_empty())
            {
                rows.texts.push(text.to_owned());
            }
        }
    }

    /// The requests made since the last call, each with its tokens, oldest first.
    pub fn take_requests(&mut self) -> Vec<Usage> {
        self.requests.drain(..).map(|(_, usage)| usage).collect()
    }
}

fn usage(usage: &Value) -> Usage {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let written = n(&usage["cache_creation_input_tokens"]);
    let long = n(&usage["cache_creation"]["ephemeral_1h_input_tokens"]);
    Usage {
        input: n(&usage["input_tokens"]),
        output: n(&usage["output_tokens"]),
        cache_read: n(&usage["cache_read_input_tokens"]),
        cache_write_short: written.saturating_sub(long),
        cache_write_long: long,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rows_are_read_as_they_are_written_and_a_request_counts_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        let row = |id: &str, block: Value, output: u64| {
            serde_json::json!({ "type": "assistant", "message": { "id": id, "content": [block],
                "usage": { "input_tokens": 10, "output_tokens": output, "cache_read_input_tokens": 3,
                    "cache_creation_input_tokens": 5, "cache_creation": { "ephemeral_1h_input_tokens": 2 } } } })
            .to_string()
        };
        let mut transcript = Transcript::new(path.clone());
        writeln!(
            file,
            "{}",
            row("m1", serde_json::json!({"type": "text", "text": "hi"}), 1)
        )
        .unwrap();
        write!(
            file,
            "{}",
            &row("m1", serde_json::json!({"type": "tool_use"}), 4)[..20]
        )
        .unwrap();
        let first = transcript.read();
        assert_eq!(first.texts, vec!["hi".to_owned()]);
        writeln!(
            file,
            "{}",
            &row("m1", serde_json::json!({"type": "tool_use"}), 4)[20..]
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            row("m2", serde_json::json!({"type": "text", "text": "done"}), 7)
        )
        .unwrap();
        assert_eq!(transcript.read().texts, vec!["done".to_owned()]);
        let requests = transcript.take_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].output, 4);
        assert_eq!(requests[0].cache_write_short, 3);
        assert_eq!(requests[0].cache_write_long, 2);
        assert!(transcript.take_requests().is_empty());
    }
}
