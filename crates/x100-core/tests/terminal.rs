//! A terminal's chunks survive the trip to JSON and back, whatever bytes the program wrote.

use x100_core::bytes::Bytes;
use x100_core::terminal::{Chunk, ChunkKind};
use x100_core::time::Timestamp;

#[test]
fn every_byte_value_and_a_resize_survive_json() {
    let all: Vec<u8> = (0..=255).collect();
    let chunks = [
        ChunkKind::Resize {
            cols: 120,
            rows: 40,
        },
        ChunkKind::Output {
            bytes: Bytes(all.clone()),
        },
        ChunkKind::Output {
            // A character cut in half and an escape sequence cut short, as a read can leave them.
            bytes: Bytes(vec![0xe2, 0x82, 0x1b, b'[']),
        },
        ChunkKind::Output {
            bytes: Bytes(vec![]),
        },
    ];
    for (i, what) in chunks.into_iter().enumerate() {
        let chunk = Chunk {
            at: Timestamp::from_millis(i as u64),
            what,
        };
        let text = serde_json::to_string(&chunk).unwrap();
        assert_eq!(serde_json::from_str::<Chunk>(&text).unwrap(), chunk);
    }
    let json = serde_json::to_value(ChunkKind::Output { bytes: Bytes(all) }).unwrap();
    assert_eq!(json["type"], "output");
    assert!(json["bytes"].is_string());
}
