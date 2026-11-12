//! Every kind of transcript item survives the trip to JSON and back.

use std::collections::BTreeSet;

use x100_core::id::DriverId;
use x100_core::money::Usage;
use x100_core::time::Timestamp;
use x100_core::transcript::{Entry, Item, TurnEnd};

fn driver(n: u128) -> DriverId {
    DriverId::from_u128(n)
}

fn items() -> Vec<Item> {
    vec![
        Item::StartPack {
            text: "ROLE: CEO\nYour id: agt_1\n".into(),
        },
        Item::TurnStarted,
        Item::Text {
            text: "hi\nthere".into(),
        },
        Item::ToolCall {
            call: "c1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls", "args": [1, 2]}),
        },
        Item::ToolOutput {
            call: "c1".into(),
            chunk: "\u{1b}[32mok\u{1b}[0m\r\n".into(),
        },
        Item::ToolResult {
            call: "c1".into(),
            output: "o".into(),
            is_error: true,
        },
        Item::Usage {
            usage: Usage {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write_short: 4,
                cache_write_long: 5,
            },
        },
        Item::TurnEnded {
            how: TurnEnd::Completed,
        },
        Item::TurnEnded {
            how: TurnEnd::Failed { detail: "x".into() },
        },
        Item::TurnEnded {
            how: TurnEnd::Interrupted,
        },
        Item::DriverChanged { from: None },
        Item::DriverChanged {
            from: Some(driver(1)),
        },
    ]
}

/// Names each variant, exhaustively, so adding a variant without a sample above fails to compile
/// here.
fn variant(item: &Item) -> &'static str {
    match item {
        Item::StartPack { .. } => "StartPack",
        Item::TurnStarted => "TurnStarted",
        Item::Text { .. } => "Text",
        Item::ToolCall { .. } => "ToolCall",
        Item::ToolOutput { .. } => "ToolOutput",
        Item::ToolResult { .. } => "ToolResult",
        Item::Usage { .. } => "Usage",
        Item::TurnEnded { .. } => "TurnEnded",
        Item::DriverChanged { .. } => "DriverChanged",
    }
}

#[test]
fn every_transcript_item_round_trips() {
    let items = items();
    let names: BTreeSet<_> = items.iter().map(variant).collect();
    assert_eq!(names.len(), 9, "a variant has no sample");
    for item in items {
        let entry = Entry {
            at: Timestamp::from_millis(5),
            driver: driver(1),
            item,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry, "{json}");
    }
}

#[test]
fn a_tool_output_is_written_as_its_call_and_chunk() {
    let item = Item::ToolOutput {
        call: "c1".into(),
        chunk: "x".into(),
    };
    assert_eq!(
        serde_json::to_value(&item).unwrap(),
        serde_json::json!({"type": "tool_output", "call": "c1", "chunk": "x"})
    );
}

#[test]
fn a_start_pack_is_written_as_its_text() {
    let item = Item::StartPack { text: "x".into() };
    assert_eq!(
        serde_json::to_value(&item).unwrap(),
        serde_json::json!({"type": "start_pack", "text": "x"})
    );
}
