//! Replacing credential values with their references in what a driver emits, before anything is
//! recorded or shown: every string in a value, whatever its shape.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use x100_core::bytes::Bytes;
use x100_core::terminal::{Chunk, ChunkKind};
use x100_core::time::Timestamp;
use x100_vault::{RedactStream, Redactor};

/// One terminal's output, redacted as it arrives: a value split across two chunks is still found,
/// so the bytes that might start one wait for the next chunk.
pub(crate) struct Screen(RedactStream);

impl Screen {
    pub fn new(redactor: &Redactor) -> Self {
        Self(redactor.stream())
    }

    /// The chunk as it may be recorded, or `None` while all its bytes are held back.
    pub fn chunk(&mut self, mut chunk: Chunk) -> Option<Chunk> {
        if let ChunkKind::Output { bytes } = &mut chunk.what {
            bytes.0 = self.0.push(&bytes.0);
            if bytes.0.is_empty() {
                return None;
            }
        }
        Some(chunk)
    }

    /// What was held back, at the end of the output.
    pub fn finish(&mut self, at: Timestamp) -> Option<Chunk> {
        let rest = self.0.finish();
        (!rest.is_empty()).then_some(Chunk {
            at,
            what: ChunkKind::Output { bytes: Bytes(rest) },
        })
    }
}

/// `value` with every string in it redacted, or `None` if the result no longer fits its type,
/// which a caller treats as a refusal rather than keep the unredacted original.
pub(crate) fn value<T: Serialize + DeserializeOwned>(redactor: &Redactor, value: &T) -> Option<T> {
    let mut json = serde_json::to_value(value).ok()?;
    strings(redactor, &mut json);
    serde_json::from_value(json).ok()
}

/// Redacts every string in place, keys included, since an object can be keyed by what a driver
/// chose.
fn strings(redactor: &Redactor, value: &mut Value) {
    match value {
        Value::String(s) => *s = redactor.redact_str(s),
        Value::Array(items) => {
            for v in items {
                strings(redactor, v);
            }
        }
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                let mut v = map.remove(&k).expect("a key just read");
                strings(redactor, &mut v);
                map.insert(redactor.redact_str(&k), v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use x100_core::id::CredentialRef;
    use x100_core::transcript::Item;
    use x100_vault::SecretValue;

    use super::*;

    #[test]
    fn every_string_of_a_value_is_redacted() {
        let secret = SecretValue::from("tok_live_0123456789abcdef");
        let reference = CredentialRef::from_u128(7);
        let r = Redactor::for_values(&[(reference, &secret)]);
        let item = Item::ToolCall {
            call: "c1".into(),
            name: "Bash".into(),
            input: serde_json::json!({
                "command": "curl -H 'Authorization: tok_live_0123456789abcdef'",
                "list": ["x tok_live_0123456789abcdef y"],
            }),
        };
        let out = value(&r, &item).unwrap();
        let text = serde_json::to_string(&out).unwrap();
        assert!(!text.contains("tok_live"), "{text}");
        assert!(text.contains(&reference.to_string()), "{text}");
    }
}
