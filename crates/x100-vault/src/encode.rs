//! The forms a value can take in output: as it is, percent-encoded by the common encoders, hex,
//! and base64 in both alphabets at each of the three alignments.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};

/// Every form of `value` the redactor looks for, without duplicates.
pub(crate) fn forms(value: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![value.to_vec()];
    for keep in [UNRESERVED, URI_COMPONENT, FORM, PATH] {
        out.push(percent(value, keep, UPPER, false));
        out.push(percent(value, keep, LOWER, false));
    }
    out.push(percent(value, FORM, UPPER, true));
    out.push(percent(value, FORM, LOWER, true));
    out.push(hex(value, LOWER));
    out.push(hex(value, UPPER));
    for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
        for offset in 0..3 {
            out.push(base64_core(engine, value, offset));
        }
    }
    out.sort();
    out.dedup();
    out
}

const UPPER: &[u8; 16] = b"0123456789ABCDEF";
const LOWER: &[u8; 16] = b"0123456789abcdef";

/// RFC 3986's unreserved characters, which every encoder leaves alone. Python's `quote(safe='')`.
const UNRESERVED: &[u8] = b"-._~";
/// JavaScript's `encodeURIComponent`.
const URI_COMPONENT: &[u8] = b"-._~!*'()";
/// HTML forms and WHATWG `application/x-www-form-urlencoded`.
const FORM: &[u8] = b"-._*";
/// Python's `quote` with its default `safe='/'`.
const PATH: &[u8] = b"-._~/";

fn percent(value: &[u8], keep: &[u8], digits: &[u8; 16], plus_for_space: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() * 3);
    for &b in value {
        if b.is_ascii_alphanumeric() || keep.contains(&b) {
            out.push(b);
        } else if b == b' ' && plus_for_space {
            out.push(b'+');
        } else {
            out.extend_from_slice(&[
                b'%',
                digits[usize::from(b >> 4)],
                digits[usize::from(b & 15)],
            ]);
        }
    }
    out
}

fn hex(value: &[u8], digits: &[u8; 16]) -> Vec<u8> {
    value
        .iter()
        .flat_map(|&b| [digits[usize::from(b >> 4)], digits[usize::from(b & 15)]])
        .collect()
}

/// The characters of base64 output that depend only on `value`, when `value` starts `offset`
/// bytes into the encoded data. The characters at either end also carry bits of the bytes around
/// it, which the redactor cannot know, so they are left out: at most one character on each side,
/// a few bits of the value's first and last byte, stays in the output.
pub(crate) fn base64_core(engine: &impl Engine, value: &[u8], offset: usize) -> Vec<u8> {
    let mut data = vec![0u8; offset];
    data.extend_from_slice(value);
    data.extend_from_slice(&[0, 0]);
    let encoded = engine.encode(&data).into_bytes();
    let first_bit = 8 * offset;
    let end_bit = 8 * (offset + value.len());
    let start = first_bit.div_ceil(6);
    let end = end_bit / 6;
    encoded[start..end].to_vec()
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::STANDARD;

    use super::*;

    #[test]
    fn a_plain_value_has_no_duplicate_forms() {
        let forms = forms(b"zyxwvuts12");
        assert!(forms.contains(&b"zyxwvuts12".to_vec()));
        assert!(forms.contains(&b"7a797877767574733132".to_vec()));
        // Every percent form is the value itself and the hex has no letters, so: the value, its
        // hex, and at most six base64 cores.
        assert!(forms.len() <= 8, "{}", forms.len());
    }

    #[test]
    fn the_base64_core_is_inside_the_encoding_at_every_alignment() {
        let value = b"s3cret-value!";
        for prefix in ["", "a", "ab", "abc", "user:"] {
            let mut data = prefix.as_bytes().to_vec();
            data.extend_from_slice(value);
            for suffix in ["", "x", "xy"] {
                let mut all = data.clone();
                all.extend_from_slice(suffix.as_bytes());
                let encoded = STANDARD.encode(&all);
                let core = base64_core(&STANDARD_NO_PAD, value, prefix.len() % 3);
                assert!(
                    encoded.contains(std::str::from_utf8(&core).unwrap()),
                    "{prefix:?} {suffix:?}: {encoded} lacks {core:?}"
                );
                assert!(core.len() >= value.len() * 4 / 3 - 2);
            }
        }
    }

    #[test]
    fn special_characters_have_each_encoders_form() {
        let forms = forms(b"a b/c+d=e");
        for expected in [
            &b"a%20b%2Fc%2Bd%3De"[..],
            b"a%20b%2fc%2bd%3de",
            b"a+b%2Fc%2Bd%3De",
            b"a%20b/c%2Bd%3De",
        ] {
            assert!(
                forms.contains(&expected.to_vec()),
                "{}",
                String::from_utf8_lossy(expected)
            );
        }
    }
}
