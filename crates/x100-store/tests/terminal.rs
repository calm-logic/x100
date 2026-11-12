mod common;

use std::thread;

use x100_core::bytes::Bytes;
use x100_core::id::DriverId;
use x100_core::terminal::{Chunk, ChunkKind, TermPos};
use x100_core::time::Timestamp;
use x100_store::ProjectStore;

fn output(i: u64, bytes: Vec<u8>) -> Chunk {
    Chunk {
        at: Timestamp::from_millis(i),
        what: ChunkKind::Output {
            bytes: Bytes(bytes),
        },
    }
}

fn resize(i: u64, cols: u16, rows: u16) -> Chunk {
    Chunk {
        at: Timestamp::from_millis(i),
        what: ChunkKind::Resize { cols, rows },
    }
}

#[test]
fn chunks_read_back_in_order_and_bytes_of_every_value_come_back_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    let (d, other) = (DriverId::from_u128(1), DriverId::from_u128(2));
    let t = &store.terminals;

    let all: Vec<u8> = (0..=255).collect();
    let script = [
        resize(1, 120, 40),
        output(2, all),
        output(3, vec![0xe2, 0x82, 0x1b, b'[']),
        resize(4, 100, 30),
        output(5, vec![]),
    ];
    for c in &script {
        t.append(d, c.clone()).unwrap();
        // Another driver's chunks do not disturb the numbering.
        t.append(other, output(0, vec![0])).unwrap();
    }

    let got = t.read(d, None, 100).unwrap();
    assert_eq!(got.len(), script.len());
    for (i, ((pos, chunk), want)) in got.iter().zip(&script).enumerate() {
        assert_eq!(pos.0, i as u64 + 1);
        assert_eq!(chunk, want);
    }
    let tail = t.read(d, Some(TermPos(3)), 100).unwrap();
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].1, resize(4, 100, 30));
    assert_eq!(t.read(d, Some(TermPos(1)), 2).unwrap().len(), 2);
    assert_eq!(t.last(d).unwrap(), Some(TermPos(5)));
    assert_eq!(t.last(DriverId::from_u128(3)).unwrap(), None);
}

#[test]
fn positions_continue_after_the_store_is_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let d = DriverId::from_u128(1);
    {
        let store = ProjectStore::open(dir.path(), clock.clone()).unwrap();
        store.terminals.append(d, resize(1, 80, 24)).unwrap();
        store
            .terminals
            .append(d, output(2, b"hi".to_vec()))
            .unwrap();
    }
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    let pos = store.terminals.append(d, output(3, b"!".to_vec())).unwrap();
    assert_eq!(pos, TermPos(3));
}

#[test]
fn following_from_a_position_gets_the_stored_chunks_then_new_ones_with_none_missed_or_repeated() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    let d = DriverId::from_u128(1);
    let t = store.terminals.clone();
    for i in 1..=20 {
        t.append(d, output(i, vec![i as u8])).unwrap();
    }
    let mut rx = t.follow(d, Some(TermPos(15))).unwrap();
    let writer = {
        let t = t.clone();
        thread::spawn(move || {
            for i in 21..=500 {
                t.append(d, output(i, vec![i as u8])).unwrap();
            }
        })
    };
    writer.join().unwrap();
    let got: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|(p, _)| p.0)
        .collect();
    assert_eq!(got, (16..=500).collect::<Vec<_>>());
}
