//! Replacing known values with their references in whatever a driver emits.
//!
//! One Aho-Corasick automaton holds every form of every value (see [`crate::encode`]), matched
//! leftmost-longest, each replaced by its credential's reference. A stream keeps back only the
//! tail that could still turn into a match once more bytes arrive.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use aho_corasick::{AhoCorasick, MatchKind};
use x100_core::id::CredentialRef;
use zeroize::Zeroizing;

use crate::encode;
use crate::secret::SecretValue;

/// The automaton and what each pattern is replaced with. The automaton's tables encode the
/// values and are not wiped when dropped: a limit of using a general-purpose matcher.
pub(crate) struct Patterns {
    matcher: Option<AhoCorasick>,
    replacement: Vec<Arc<str>>,
    /// Every pattern, sorted, to answer "is this tail the start of a pattern" by binary search.
    sorted: Vec<Zeroizing<Vec<u8>>>,
    longest: usize,
}

impl Patterns {
    pub(crate) fn build<'a>(
        values: impl IntoIterator<Item = (CredentialRef, &'a SecretValue)>,
    ) -> Self {
        let mut pairs: Vec<(Zeroizing<Vec<u8>>, Arc<str>)> = Vec::new();
        for (reference, value) in values {
            let name: Arc<str> = reference.to_string().into();
            for form in encode::forms(value.expose()) {
                pairs.push((Zeroizing::new(form), Arc::clone(&name)));
            }
        }
        let longest = pairs.iter().map(|(p, _)| p.len()).max().unwrap_or(0);
        let matcher = (!pairs.is_empty()).then(|| {
            AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .build(pairs.iter().map(|(p, _)| p.as_slice()))
                .expect("literal patterns always build")
        });
        let replacement = pairs.iter().map(|(_, r)| Arc::clone(r)).collect();
        let mut sorted: Vec<_> = pairs.into_iter().map(|(p, _)| p).collect();
        sorted.sort_by(|a, b| a.as_slice().cmp(b.as_slice()));
        Self {
            matcher,
            replacement,
            sorted,
            longest,
        }
    }

    fn starts_a_pattern(&self, tail: &[u8]) -> bool {
        let i = self.sorted.partition_point(|p| p.as_slice() < tail);
        self.sorted.get(i).is_some_and(|p| p.starts_with(tail))
    }

    /// Redacts `buf` into `out` as far as the bytes seen decide it, and returns how many bytes of
    /// `buf` were used. With `last` everything is decided.
    fn run(&self, buf: &[u8], last: bool, out: &mut Vec<u8>) -> usize {
        let Some(matcher) = &self.matcher else {
            out.extend_from_slice(buf);
            return buf.len();
        };
        // A pattern starting before `decided` has all its bytes in `buf` already.
        let decided = if last {
            buf.len()
        } else {
            buf.len().saturating_sub(self.longest - 1)
        };
        let mut pos = 0;
        let mut next_match = buf.len();
        for m in matcher.find_iter(buf) {
            if m.start() >= decided {
                next_match = m.start();
                break;
            }
            out.extend_from_slice(&buf[pos..m.start()]);
            out.extend_from_slice(self.replacement[m.pattern().as_usize()].as_bytes());
            pos = m.end();
        }
        if last {
            out.extend_from_slice(&buf[pos..]);
            return buf.len();
        }
        let from = pos.max(decided);
        let keep = (from..next_match)
            .find(|&p| self.starts_a_pattern(&buf[p..]))
            .unwrap_or(next_match)
            .max(pos);
        out.extend_from_slice(&buf[pos..keep]);
        keep
    }
}

/// What the vault shares with every [`Redactor`]: the current patterns, replaced whenever a
/// credential is added, replaced or removed.
pub(crate) struct Shared {
    current: RwLock<Arc<Patterns>>,
    generation: AtomicU64,
}

impl Shared {
    pub(crate) fn new(patterns: Patterns) -> Self {
        Self {
            current: RwLock::new(Arc::new(patterns)),
            generation: AtomicU64::new(0),
        }
    }

    pub(crate) fn replace(&self, patterns: Patterns) {
        *self.current.write().expect("redactor lock") = Arc::new(patterns);
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self) -> (Arc<Patterns>, u64) {
        let generation = self.generation.load(Ordering::Acquire);
        let patterns = Arc::clone(&self.current.read().expect("redactor lock"));
        (patterns, generation)
    }
}

/// Replaces every known value, in any of its forms, with its credential's reference. Cheap to
/// clone, and always uses the vault's current credentials.
#[derive(Clone)]
pub struct Redactor(pub(crate) Arc<Shared>);

impl Redactor {
    /// For tests of code that redacts: a redactor that knows only `values`.
    pub fn for_values(values: &[(CredentialRef, &SecretValue)]) -> Self {
        Self(Arc::new(Shared::new(Patterns::build(
            values.iter().map(|(r, v)| (*r, *v)),
        ))))
    }

    pub fn redact(&self, bytes: &[u8]) -> Vec<u8> {
        let (patterns, _) = self.0.snapshot();
        let mut out = Vec::with_capacity(bytes.len());
        patterns.run(bytes, true, &mut out);
        out
    }

    /// Text in, text out. A value that is not itself UTF-8 could split a character when
    /// replaced, so such a result is made valid with replacement characters.
    pub fn redact_str(&self, text: &str) -> String {
        let out = self.redact(text.as_bytes());
        String::from_utf8(out)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// A stream redactor for output that arrives in chunks, such as a terminal or a response
    /// body.
    pub fn stream(&self) -> RedactStream {
        let (patterns, generation) = self.0.snapshot();
        RedactStream {
            shared: Arc::clone(&self.0),
            patterns,
            generation,
            pending: Vec::new(),
        }
    }
}

/// Redacts a stream chunk by chunk. A value split across chunks is still found: the stream holds
/// back the shortest tail that could be the start of a value's form, and nothing more.
pub struct RedactStream {
    shared: Arc<Shared>,
    patterns: Arc<Patterns>,
    generation: u64,
    pending: Vec<u8>,
}

impl RedactStream {
    /// The redacted bytes that can be released now.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.shared.generation.load(Ordering::Acquire) != self.generation {
            (self.patterns, self.generation) = self.shared.snapshot();
        }
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(self.pending.len());
        let used = self.patterns.run(&self.pending, false, &mut out);
        self.pending.drain(..used);
        out
    }

    /// Releases what was held back, at the end of the stream.
    pub fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pending.len());
        self.patterns.run(&self.pending, true, &mut out);
        self.pending.clear();
        out
    }

    /// How many bytes are held back, waiting to see whether they start a value.
    pub fn held(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor(values: &[&str]) -> (Redactor, Vec<CredentialRef>) {
        let secrets: Vec<SecretValue> = values.iter().map(|v| SecretValue::from(*v)).collect();
        let refs: Vec<CredentialRef> = (0..values.len() as u128)
            .map(|n| CredentialRef::from_u128(n + 1))
            .collect();
        let pairs: Vec<_> = refs.iter().copied().zip(secrets.iter()).collect();
        (Redactor::for_values(&pairs), refs)
    }

    #[test]
    fn a_value_and_its_forms_become_the_reference() {
        let (r, refs) = redactor(&["hunter2-hunter2/+"]);
        let reference = refs[0].to_string();
        for form in [
            "hunter2-hunter2/+",
            "hunter2-hunter2%2F%2B",
            "68756e746572322d68756e746572322f2b",
        ] {
            let out = r.redact_str(&format!("token={form}; done"));
            assert_eq!(out, format!("token={reference}; done"));
        }
    }

    #[test]
    fn nothing_known_passes_untouched() {
        let (r, _) = redactor(&[]);
        assert_eq!(r.redact_str("anything at all"), "anything at all");
        let mut s = r.stream();
        assert_eq!(s.push(b"abc"), b"abc");
        assert_eq!(s.held(), 0);
    }

    #[test]
    fn a_stream_holds_back_only_a_possible_start() {
        let (r, refs) = redactor(&["supersecretvalue"]);
        let mut s = r.stream();
        assert_eq!(s.push(b"plain text "), b"plain text ");
        assert_eq!(s.held(), 0);
        assert_eq!(s.push(b"then super"), b"then ");
        assert_eq!(s.held(), 5);
        let mut out = s.push(b"secretvalue and more");
        out.extend(s.finish());
        assert_eq!(out, format!("{} and more", refs[0]).into_bytes());
    }

    #[test]
    fn a_false_start_is_released_once_it_cannot_match() {
        let (r, _) = redactor(&["supersecretvalue"]);
        let mut s = r.stream();
        assert_eq!(s.push(b"supers"), b"");
        assert_eq!(s.push(b"tition"), b"superstition");
        assert_eq!(s.held(), 0);
    }

    #[test]
    fn the_longest_of_two_overlapping_values_wins_across_chunks() {
        let (r, refs) = redactor(&["secret12345", "secret123456789"]);
        let mut s = r.stream();
        let mut out = s.push(b"x secret12345");
        out.extend(s.push(b"6789 y"));
        out.extend(s.finish());
        assert_eq!(out, format!("x {} y", refs[1]).into_bytes());
    }
}
