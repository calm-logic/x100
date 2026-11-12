//! The redactor never lets a value through, in any form it knows and at any chunk split, and is
//! fast enough to sit on every byte a driver emits.

mod common;

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use proptest::prelude::*;
use x100_core::id::CredentialRef;
use x100_vault::{Redactor, SecretValue};

/// How a value is written into the output.
#[derive(Debug, Clone, Copy)]
enum Written {
    /// One of the test's own forms, by index into `common::forms`.
    Form(usize),
    /// Inside other bytes, then the whole thing base64-encoded.
    Base64 {
        url: bool,
        before: usize,
        after: usize,
    },
}

fn values() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 8..48),
        prop::collection::vec(
            prop::sample::select(b"abcdefXYZ0123456789-_.~+/=%&?:@ ".to_vec()),
            8..48
        ),
    ]
}

fn written() -> impl Strategy<Value = Written> {
    prop_oneof![
        (0usize..64).prop_map(Written::Form),
        (any::<bool>(), 0usize..6, 0usize..6).prop_map(|(url, before, after)| Written::Base64 {
            url,
            before,
            after
        }),
    ]
}

/// The output with the value written in, and the stretch of it that must not survive.
fn write(value: &[u8], how: Written, filler: &[u8]) -> (Vec<u8>, Vec<u8>) {
    match how {
        Written::Form(i) => {
            // Whole forms only: fragments at an alignment are the other branch's job.
            let forms: Vec<_> = common::forms(value)
                .into_iter()
                .filter(|(name, _)| !name.contains("after"))
                .collect();
            let (_, form) = forms[i % forms.len()].clone();
            let mut out = b"log: ".to_vec();
            out.extend_from_slice(&form);
            out.extend_from_slice(b" end");
            (out, form)
        }
        Written::Base64 { url, before, after } => {
            let mut data: Vec<u8> = filler.iter().cycle().take(before).copied().collect();
            data.extend_from_slice(value);
            data.extend(filler.iter().cycle().take(after));
            let engine = if url { &URL_SAFE } else { &STANDARD };
            let encoded = engine.encode(&data).into_bytes();
            // Characters two away from the value's edges depend on the value alone.
            let first = (8 * before).div_ceil(6) + 2;
            let last = (8 * (before + value.len())) / 6 - 2;
            let core = encoded[first..last].to_vec();
            let mut out = b"Authorization: Basic ".to_vec();
            out.extend_from_slice(&encoded);
            out.extend_from_slice(b"\r\n");
            (out, core)
        }
    }
}

fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    for i in memchr::memmem::find_iter(haystack, from) {
        out.extend_from_slice(&haystack[at..i]);
        out.extend_from_slice(to);
        at = i + from.len();
    }
    out.extend_from_slice(&haystack[at..]);
    out
}

fn stream(redactor: &Redactor, input: &[u8], splits: &[usize]) -> Vec<u8> {
    let mut points: Vec<usize> = splits.iter().map(|s| s % (input.len() + 1)).collect();
    points.sort_unstable();
    let mut s = redactor.stream();
    let mut out = Vec::new();
    let mut at = 0;
    for p in points {
        out.extend(s.push(&input[at..p]));
        at = p;
    }
    out.extend(s.push(&input[at..]));
    out.extend(s.finish());
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn no_value_gets_through(
        value in values(),
        others in prop::collection::vec(values(), 0..3),
        how in written(),
        filler in prop::collection::vec(any::<u8>(), 1..8),
        splits in prop::collection::vec(any::<usize>(), 0..6),
    ) {
        let secret = SecretValue::new(value.clone());
        let other_secrets: Vec<SecretValue> = others.iter().map(|o| SecretValue::new(o.clone())).collect();
        let mut pairs = vec![(CredentialRef::from_u128(1), &secret)];
        for (i, o) in other_secrets.iter().enumerate() {
            pairs.push((CredentialRef::from_u128(i as u128 + 2), o));
        }
        let redactor = Redactor::for_values(&pairs);
        let (input, must_go) = write(&value, how, &filler);
        let out = stream(&redactor, &input, &splits);
        // Streamed and whole agree.
        prop_assert_eq!(&out, &redactor.redact(&input));
        // The references put in are not the value, whatever digits they share with it.
        let mut text = out;
        for (reference, _) in &pairs {
            text = replace(&text, reference.to_string().as_bytes(), b"<REF>");
        }
        prop_assert!(memchr::memmem::find(&text, &must_go).is_none(), "{how:?} survived in {:?}", String::from_utf8_lossy(&text));
        prop_assert!(memchr::memmem::find(&text, &value).is_none());
    }
}

/// Fifty credentials shaped like API keys, a mebibyte of log-like output with twenty of them
/// written in, raw or encoded. Run with `--release -- --nocapture` for the figures.
#[test]
fn one_mebibyte_with_fifty_credentials() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let values: Vec<Vec<u8>> = (0..50)
        .map(|_| {
            let len = 32 + (next() % 17) as usize;
            let mut v = b"sk_test_".to_vec();
            v.extend((0..len).map(|_| alphabet[(next() % 64) as usize]));
            v
        })
        .collect();
    let secrets: Vec<SecretValue> = values.iter().map(|v| SecretValue::new(v.clone())).collect();
    let pairs: Vec<_> = secrets
        .iter()
        .enumerate()
        .map(|(i, s)| (CredentialRef::from_u128(i as u128 + 1), s))
        .collect();
    let started = Instant::now();
    let redactor = Redactor::for_values(&pairs);
    let build = started.elapsed();

    let words = [
        "compiling",
        "x100-core",
        "warning:",
        "unused",
        "\x1b[32mok\x1b[0m",
        "test",
        "result",
        "GET /api/v1/items 200",
    ];
    let mut input = Vec::with_capacity(1 << 20);
    let mut planted = 0;
    while input.len() < (1 << 20) - 200 {
        if input.len() > planted * 50_000 && planted < 20 {
            let v = &values[planted * 2];
            match planted % 3 {
                0 => input.extend_from_slice(v),
                1 => input.extend(STANDARD.encode(v).into_bytes()),
                _ => input.extend(
                    v.iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                        .into_bytes(),
                ),
            }
            planted += 1;
        }
        input.extend_from_slice(words[(next() % words.len() as u64) as usize].as_bytes());
        input.push(if next() % 8 == 0 { b'\n' } else { b' ' });
    }
    input.resize(1 << 20, b'.');
    assert_eq!(planted, 20);

    let rounds = 10;
    let mut whole = Vec::new();
    let mut streamed = Vec::new();
    let mut out_whole = Vec::new();
    let mut out_stream = Vec::new();
    for _ in 0..rounds {
        let t = Instant::now();
        out_whole = redactor.redact(&input);
        whole.push(t.elapsed());
        let t = Instant::now();
        let mut s = redactor.stream();
        out_stream.clear();
        for chunk in input.chunks(4096) {
            out_stream.extend(s.push(chunk));
        }
        out_stream.extend(s.finish());
        streamed.push(t.elapsed());
    }
    assert_eq!(out_whole, out_stream);
    for v in &values {
        assert!(memchr::memmem::find(&out_whole, v).is_none());
    }
    let replaced = out_whole.windows(4).filter(|w| w == b"ref_").count();
    assert_eq!(replaced, 20);
    whole.sort();
    streamed.sort();
    let mbps = |d: Duration| (1.0 / d.as_secs_f64()) as u64;
    println!(
        "redaction, 1 MiB, 50 credentials: automaton built in {build:?}; whole: median {:?} ({} MiB/s); \
         4 KiB chunks: median {:?} ({} MiB/s)",
        whole[rounds / 2],
        mbps(whole[rounds / 2]),
        streamed[rounds / 2],
        mbps(streamed[rounds / 2]),
    );
}
