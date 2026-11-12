mod common;

use std::thread;

use x100_core::id::{AgentId, DriverId};
use x100_core::money::Usage;
use x100_core::time::Timestamp;
use x100_store::ProjectStore;
use x100_store::transcript::{Entry, Item, Pos, TurnEnd};

fn entry(driver: DriverId, i: u64, item: Item) -> Entry {
    Entry {
        at: Timestamp::from_millis(i),
        driver,
        item,
    }
}

fn text(driver: DriverId, i: u64) -> Entry {
    entry(
        driver,
        i,
        Item::Text {
            text: format!("line {i}"),
        },
    )
}

#[test]
fn a_turn_across_two_drivers_reads_back_in_order_with_the_change_marked() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    let (agent, other) = (AgentId::from_u128(1), AgentId::from_u128(2));
    let (first, second) = (DriverId::from_u128(1), DriverId::from_u128(2));
    let t = &store.transcripts;

    let script = [
        entry(first, 1, Item::DriverChanged { from: None }),
        entry(first, 2, Item::TurnStarted),
        entry(
            first,
            3,
            Item::Text {
                text: "reading".into(),
            },
        ),
        entry(
            first,
            4,
            Item::ToolCall {
                call: "c1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
        ),
        entry(
            first,
            5,
            Item::ToolResult {
                call: "c1".into(),
                output: "a\nb".into(),
                is_error: false,
            },
        ),
        entry(
            first,
            6,
            Item::Usage {
                usage: Usage {
                    input: 10,
                    output: 2,
                    ..Usage::default()
                },
            },
        ),
        entry(
            first,
            7,
            Item::TurnEnded {
                how: TurnEnd::Failed {
                    detail: "stream broke".into(),
                },
            },
        ),
        entry(second, 8, Item::DriverChanged { from: Some(first) }),
        entry(second, 9, Item::TurnStarted),
    ];
    for e in &script {
        t.append(agent, e.clone()).unwrap();
        // Another agent's entries do not disturb the numbering.
        t.append(other, text(first, 0)).unwrap();
    }

    let all = t.read(agent, None, 100).unwrap();
    assert_eq!(all.len(), script.len());
    for (i, ((pos, got), want)) in all.iter().zip(&script).enumerate() {
        assert_eq!(pos.0, i as u64 + 1);
        assert_eq!(got, want);
    }
    let tail = t.read(agent, Some(Pos(7)), 100).unwrap();
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].1.item, Item::DriverChanged { from: Some(first) });
    assert_eq!(t.read(agent, Some(Pos(2)), 3).unwrap().len(), 3);
    assert_eq!(t.last(agent).unwrap(), Some(Pos(9)));
    assert_eq!(t.last(AgentId::from_u128(3)).unwrap(), None);
}

#[test]
fn positions_continue_after_the_store_is_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let agent = AgentId::from_u128(1);
    let d = DriverId::from_u128(1);
    {
        let store = ProjectStore::open(dir.path(), clock.clone()).unwrap();
        store.transcripts.append(agent, text(d, 1)).unwrap();
        store.transcripts.append(agent, text(d, 2)).unwrap();
    }
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    assert_eq!(store.transcripts.append(agent, text(d, 3)).unwrap(), Pos(3));
}

#[test]
fn following_from_a_position_gets_the_stored_entries_then_new_ones_with_none_missed_or_repeated() {
    let dir = tempfile::tempdir().unwrap();
    let (_, clock) = common::clock();
    let store = ProjectStore::open(dir.path(), clock).unwrap();
    let agent = AgentId::from_u128(1);
    let d = DriverId::from_u128(1);
    let t = store.transcripts.clone();
    for i in 1..=20 {
        t.append(agent, text(d, i)).unwrap();
    }
    let mut rx = t.follow(agent, Some(Pos(15))).unwrap();
    let writer = {
        let t = t.clone();
        thread::spawn(move || {
            for i in 21..=500 {
                t.append(agent, text(d, i)).unwrap();
            }
        })
    };
    writer.join().unwrap();
    let got: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|(p, _)| p.0)
        .collect();
    assert_eq!(got, (16..=500).collect::<Vec<_>>());
}

/// The wire type in `x100-proto` mirrors this one because clients cannot depend on the store.
#[test]
fn every_kind_of_entry_serialises_into_the_protocols_own_type_and_back() {
    let d = DriverId::from_u128(1);
    let items = [
        Item::TurnStarted,
        Item::Text { text: "t".into() },
        Item::ToolCall {
            call: "c".into(),
            name: "n".into(),
            input: serde_json::json!({"a": [1, 2]}),
        },
        Item::ToolResult {
            call: "c".into(),
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
        Item::DriverChanged { from: Some(d) },
    ];
    for item in items {
        let mine = entry(d, 5, item);
        let json = serde_json::to_value(&mine).unwrap();
        let theirs: x100_proto::TranscriptEntry = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&theirs).unwrap(), json);
        assert_eq!(serde_json::from_value::<Entry>(json).unwrap(), mine);
    }
}
