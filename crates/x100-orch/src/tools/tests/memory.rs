//! The memory and log tools' plans: what the folded state alone decides.

use x100_core::event::{Envelope, Seq};
use x100_core::fold::ProjectState;
use x100_core::memory::NoteName;
use x100_core::tool::ToolCall;

use super::scene;
use crate::tools::MemoryPlan;

fn remember(note: &str, sources: Vec<Seq>) -> ToolCall {
    ToolCall::Remember {
        note: NoteName(note.into()),
        body: "KV.".into(),
        sources,
    }
}

#[test]
fn memory_calls_plan_the_file_work_and_leave_the_events_to_it() {
    let s = scene();
    let plan = s
        .call(s.f.tech, &remember("Auth", vec![Seq(1)]))
        .expect("allowed");
    assert!(plan.events.is_empty(), "the commit is not known yet");
    assert_eq!(
        plan.memory,
        Some(MemoryPlan::Remember {
            note: NoteName("auth.md".into()),
            body: "KV.".into(),
            sources: vec![Seq(1)],
        })
    );
    let search = ToolCall::SearchLog {
        query: "  vite  ".into(),
        limit: Some(500),
    };
    let plan = s.call(s.f.backend, &search).expect("any agent may search");
    assert!(matches!(
        plan.memory,
        Some(MemoryPlan::SearchLog { limit: 50, .. })
    ));
}

#[test]
fn memory_calls_refuse_what_the_state_alone_rules_out() {
    let s = scene();
    assert!(
        s.refused(s.f.tech, &remember("../auth", vec![]))
            .contains("not a note name")
    );
    let past = Seq(s.state.seq().unwrap().0 + 100);
    assert!(
        s.refused(s.f.tech, &remember("auth", vec![past]))
            .contains("not in the log")
    );
    assert!(
        s.refused(s.f.tech, &remember("auth", vec![Seq(0)]))
            .contains("not in the log")
    );
    let empty = ToolCall::SearchLog {
        query: "   ".into(),
        limit: None,
    };
    assert!(s.refused(s.f.tech, &empty).contains("no words"));
    // Another agent's skill is not for loading, and only the CEO reads others' memories.
    let other = ToolCall::LoadSkill {
        skill: x100_core::memory::SkillRef {
            scope: x100_core::memory::SkillScope::Agent(s.f.backend),
            name: x100_core::memory::SkillName("deploy".into()),
        },
    };
    assert!(s.refused(s.f.tech, &other).contains("belongs to"));
    let read = ToolCall::ReadMemories { agent: s.f.tech };
    assert!(s.refused(s.f.backend, &read).contains("only the CEO"));
    assert!(s.call(s.f.ceo, &read).is_ok());
}

#[test]
fn memory_needs_a_code_directory() {
    let mut s = scene();
    // The same history without the event that gives the project its directory, renumbered.
    let log: Vec<_> =
        s.f.log
            .iter()
            .filter(|e| !matches!(e.event, x100_core::event::ProjectEvent::CodeDirSet { .. }))
            .enumerate()
            .map(|(i, e)| Envelope {
                seq: Seq(i as u64 + 1),
                ..e.clone()
            })
            .collect();
    s.state = ProjectState::fold(&log).expect("the history folds without it");
    assert!(s.state.project().unwrap().code_dir.is_none());
    for call in [
        remember("auth", vec![]),
        ToolCall::ReadNote {
            note: NoteName("auth".into()),
        },
    ] {
        assert!(s.refused(s.f.tech, &call).contains("set up"), "{call:?}");
    }
    // The log can still be searched.
    let search = ToolCall::SearchLog {
        query: "vite".into(),
        limit: None,
    };
    assert!(s.call(s.f.tech, &search).is_ok());
}
