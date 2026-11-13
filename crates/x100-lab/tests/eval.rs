//! Checking and completing an eval set against core's Fernleaf scenario.

use std::collections::BTreeMap;

use x100_core::fold::ProjectState;
use x100_core::git::CommitId;
use x100_core::memory::{NoteName, SkillScope};
use x100_core::task::{Outcome, TaskState};
use x100_core::testing::{deploy_skill, fernleaf};
use x100_core::trial::{Check, EvalItem, EvalSet, TrialChange};
use x100_lab::eval::{Facts, answered, build, runs_as, started_at};

fn edit() -> TrialChange {
    TrialChange::EditSkill {
        skill: deploy_skill(),
        body: String::new(),
    }
}

fn set(items: Vec<EvalItem>, runs: u8) -> EvalSet {
    EvalSet {
        items,
        runs_per_arm: runs,
    }
}

fn past(task: x100_core::id::TaskId) -> EvalItem {
    EvalItem::PastTask {
        task,
        checkpoint: CommitId("whatever the CEO said".into()),
    }
}

fn script() -> EvalItem {
    EvalItem::Check(Check::Script {
        command: "  test -f done.txt  ".into(),
    })
}

#[test]
fn a_past_task_gets_the_checkpoint_it_started_at_and_the_runs_are_clamped() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let accepted = state
        .tasks()
        .find(|t| matches!(&t.state, TaskState::Done(_, r) if r.outcome == Outcome::Completed))
        .expect("an accepted task")
        .task
        .id;
    let started = BTreeMap::from([(accepted, CommitId("c0ffee".into()))]);
    let facts = Facts {
        state: &state,
        started_at: &started,
        notes: "",
    };
    let built = build(&edit(), &set(vec![past(accepted), script()], 1), &facts).unwrap();
    assert_eq!(built.runs_per_arm, 3, "at least three runs of each arm");
    assert_eq!(
        built.items,
        vec![
            EvalItem::PastTask {
                task: accepted,
                checkpoint: CommitId("c0ffee".into())
            },
            EvalItem::Check(Check::Script {
                command: "test -f done.txt".into()
            }),
        ]
    );
    assert_eq!(
        build(&edit(), &set(vec![past(accepted)], 200), &facts)
            .unwrap()
            .runs_per_arm,
        10
    );

    // Who it runs as: the agent that did the task, for a shared skill.
    let assignee = state
        .task(accepted)
        .unwrap()
        .state
        .assignment()
        .unwrap()
        .assignee;
    assert_eq!(runs_as(&state, &edit(), &built.items[0]), Some(assignee));

    // Without a recorded start it cannot be replayed.
    let none = BTreeMap::new();
    let facts = Facts {
        state: &state,
        started_at: &none,
        notes: "",
    };
    let e = build(&edit(), &set(vec![past(accepted)], 3), &facts).unwrap_err();
    assert!(e.contains("no checkpoint"), "{e}");
}

#[test]
fn what_cannot_be_tried_is_refused_with_what_to_change() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    let open = state
        .tasks()
        .find(|t| matches!(t.state, TaskState::InProgress(_)))
        .unwrap()
        .task
        .id;
    let started = BTreeMap::new();
    let facts = Facts {
        state: &state,
        started_at: &started,
        notes: "Deploys go through make ship.",
    };
    let refused = |change: &TrialChange, items: Vec<EvalItem>| {
        build(change, &set(items, 3), &facts).unwrap_err()
    };
    assert!(refused(&edit(), vec![]).contains("needs something to run"));
    assert!(refused(&edit(), vec![past(open)]).contains("not accepted"));
    assert!(refused(&edit(), vec![script()]).contains("run on the result of a past task"));
    let probe = |answer: &str| EvalItem::Probe {
        question: "How do deploys go?".into(),
        answer: answer.into(),
    };
    assert!(refused(&edit(), vec![probe("make ship")]).contains("try a memory compaction"));
    let compact = TrialChange::CompactMemory {
        agent: f.ceo,
        notes: vec![(NoteName("deploys".into()), "make ship".into())],
    };
    assert!(refused(&compact, vec![probe("by hand")]).contains("not in the notes"));
    let fine = build(&compact, &set(vec![probe("Make  SHIP")], 3), &facts).unwrap();
    assert_eq!(fine.items.len(), 1);
    assert_eq!(runs_as(&state, &compact, &fine.items[0]), Some(f.ceo));
    let own = TrialChange::NewSkill {
        skill: x100_core::memory::SkillRef {
            scope: SkillScope::Agent(f.tech),
            name: deploy_skill().name,
        },
        body: String::new(),
    };
    assert_eq!(runs_as(&state, &own, &past(open)), Some(f.tech));
}

#[test]
fn an_answer_is_said_when_its_words_appear_in_order() {
    assert!(answered(
        "make ship",
        "Deploys go through `make ship`, never by hand."
    ));
    assert!(answered("Box-7", "the host is box 7."));
    assert!(!answered("ship make", "make ship"));
    assert!(!answered("ship", "shipping"));
    assert!(!answered("", "anything"));
}

#[test]
fn a_task_started_where_its_assignee_last_recorded_its_worktree() {
    let f = fernleaf();
    let state = ProjectState::fold(&f.log).unwrap();
    // Every accepted task that the log records a commit before is found; one with none is not.
    let mut found = 0;
    for t in state.tasks() {
        let Some(a) = t.state.assignment() else {
            continue;
        };
        if let Some(c) = started_at(&f.log, t.task.id, a.assignee) {
            assert!(!c.0.is_empty());
            found += 1;
        }
    }
    assert!(
        found > 0,
        "Fernleaf records commits before some task starts"
    );
}
