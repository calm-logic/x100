//! Finding references (`ref_` and a ULID) in what a driver sends, and swapping them for values.

use x100_core::id::CredentialRef;

const PREFIX: &[u8] = b"ref_";
/// `ref_` and the 26 characters of a ULID.
pub(crate) const LEN: usize = 30;

/// What becomes of one reference: its value goes in, or it stays as it is.
pub(crate) trait Decide {
    fn decide(&mut self, reference: CredentialRef, out: &mut Vec<u8>) -> bool;
}

fn ulid_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// A reference stands alone: the bytes on either side are not letters or digits.
fn boundary(b: u8) -> bool {
    !b.is_ascii_alphanumeric()
}

/// Swaps references in a stream of bytes, chunk by chunk. A reference split across chunks is
/// still found: only a tail that could be the start of one is held back.
#[derive(Default)]
pub(crate) struct Swapper {
    pending: Vec<u8>,
    /// The byte before `pending`, for the boundary check.
    before: Option<u8>,
}

impl Swapper {
    pub(crate) fn push(&mut self, chunk: &[u8], decide: &mut impl Decide) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(self.pending.len());
        let used = run(&self.pending, self.before, false, &mut out, decide);
        if used > 0 {
            self.before = Some(self.pending[used - 1]);
        }
        self.pending.drain(..used);
        out
    }

    pub(crate) fn finish(&mut self, decide: &mut impl Decide) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pending.len());
        run(&self.pending, self.before, true, &mut out, decide);
        self.pending.clear();
        out
    }
}

/// Swaps every reference in a whole piece of text, such as a header value or a buffered body.
pub(crate) fn swap_all(bytes: &[u8], decide: &mut impl Decide) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    run(bytes, None, true, &mut out, decide);
    out
}

fn run(
    buf: &[u8],
    before: Option<u8>,
    last: bool,
    out: &mut Vec<u8>,
    decide: &mut impl Decide,
) -> usize {
    let mut pos = 0;
    let mut search = 0;
    while let Some(found) = memchr::memmem::find(&buf[search..], PREFIX) {
        let i = search + found;
        search = i + 1;
        let prev = if i == 0 { before } else { Some(buf[i - 1]) };
        if !prev.is_none_or(boundary) {
            continue;
        }
        let body = &buf[i + PREFIX.len()..buf.len().min(i + LEN)];
        if !body.iter().all(|&b| ulid_char(b)) {
            continue;
        }
        let complete = buf.len() > i + LEN || (last && buf.len() == i + LEN);
        if !complete {
            if last {
                continue;
            }
            // Could still become a reference: keep it for the next chunk.
            out.extend_from_slice(&buf[pos..i]);
            return i;
        }
        if buf.get(i + LEN).is_some_and(|&b| !boundary(b)) {
            continue;
        }
        let Ok(reference) = std::str::from_utf8(&buf[i..i + LEN])
            .expect("checked ASCII")
            .parse::<CredentialRef>()
        else {
            continue;
        };
        out.extend_from_slice(&buf[pos..i]);
        if !decide.decide(reference, out) {
            out.extend_from_slice(&buf[i..i + LEN]);
        }
        pos = i + LEN;
        search = pos;
    }
    if last {
        out.extend_from_slice(&buf[pos..]);
        return buf.len();
    }
    // A tail such as `re` might be the start of `ref_`.
    let keep = (1..PREFIX.len())
        .rev()
        .find(|&k| buf.len() >= pos + k && buf.ends_with(&PREFIX[..k]))
        .map_or(buf.len(), |k| buf.len() - k);
    out.extend_from_slice(&buf[pos..keep]);
    keep
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct Table(HashMap<CredentialRef, &'static str>, Vec<CredentialRef>);

    impl Decide for Table {
        fn decide(&mut self, reference: CredentialRef, out: &mut Vec<u8>) -> bool {
            self.1.push(reference);
            match self.0.get(&reference) {
                Some(v) => {
                    out.extend_from_slice(v.as_bytes());
                    true
                }
                None => false,
            }
        }
    }

    fn table() -> (Table, String, String) {
        let known = CredentialRef::from_u128(0xabc);
        let unknown = CredentialRef::from_u128(0xdef);
        (
            Table(HashMap::from([(known, "VALUE")]), Vec::new()),
            known.to_string(),
            unknown.to_string(),
        )
    }

    #[test]
    fn known_references_are_swapped_and_others_left() {
        let (mut t, known, unknown) = table();
        let text = format!("Bearer {known}, {unknown}; x{known} {known}y");
        let out = swap_all(text.as_bytes(), &mut t);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("Bearer VALUE, {unknown}; x{known} {known}y")
        );
        assert_eq!(t.1.len(), 2);
    }

    #[test]
    fn a_reference_split_at_every_point_is_swapped() {
        let (_, known, _) = table();
        let text = format!("{{\"key\":\"{known}\"}}");
        for split in 0..=text.len() {
            for second in split..=text.len() {
                let (mut t, ..) = table();
                let mut s = Swapper::default();
                let mut out = s.push(&text.as_bytes()[..split], &mut t);
                out.extend(s.push(&text.as_bytes()[split..second], &mut t));
                out.extend(s.push(&text.as_bytes()[second..], &mut t));
                out.extend(s.finish(&mut t));
                assert_eq!(
                    String::from_utf8(out).unwrap(),
                    "{\"key\":\"VALUE\"}",
                    "{split} {second}"
                );
            }
        }
    }

    #[test]
    fn a_stream_without_references_is_not_held() {
        let (mut t, ..) = table();
        let mut s = Swapper::default();
        assert_eq!(s.push(b"hello world", &mut t), b"hello world");
        assert_eq!(s.push(b" and r", &mut t), b" and ");
        assert_eq!(s.push(b"eference", &mut t), b"reference");
    }
}
