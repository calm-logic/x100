//! What the lab reads and merges, against real git in a temporary directory: a change on trial is
//! read where it is and main elsewhere, a compaction is measured and turned into note changes, and
//! a compaction merges note by note around what the agent wrote on main meanwhile.

use x100_core::event::Seq;
use x100_core::id::AgentId;
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope};
use x100_core::time::Timestamp;
use x100_memory::{Error, Memory, NoteChange, Target};

fn agent() -> AgentId {
    AgentId::from_u128(7)
}

fn note(name: &str) -> NoteName {
    NoteName(name.into())
}

fn at(ms: u64) -> Timestamp {
    Timestamp::from_millis(ms)
}

fn branch() -> Target {
    Target::Branch("trial/t1".into())
}

fn three_notes(mem: &Memory) {
    for (i, (name, body)) in [
        ("deploys", "Deploys go through `make ship`, never by hand."),
        ("deploy-host", "The deploy host is box-7."),
        ("lunch", "The builder eats at noon."),
    ]
    .into_iter()
    .enumerate()
    {
        mem.remember(agent(), &note(name), body, &[Seq(10 + i as u64)], at(1000))
            .unwrap();
    }
}

#[test]
fn a_change_on_trial_is_read_on_its_branch_and_main_is_read_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    three_notes(&mem);
    let rewrite = NoteChange::Write {
        name: note("lunch"),
        body: "Lunch is at one.".into(),
        sources: vec![Seq(30)],
    };
    mem.change_notes(agent(), &branch(), &[rewrite], at(2000), "trial")
        .unwrap();
    assert_eq!(
        mem.note_on(agent(), &note("lunch"), &branch())
            .unwrap()
            .body,
        "Lunch is at one."
    );
    assert_eq!(
        mem.note(agent(), &note("lunch")).unwrap().body,
        "The builder eats at noon."
    );
    // Another agent has no such branch, so it is read at main.
    let other = AgentId::from_u128(8);
    mem.remember(other, &note("x"), "y", &[], at(1)).unwrap();
    assert_eq!(mem.notes_on(other, &branch()).unwrap().len(), 1);

    // The shared skills have no trial branch: a skill is read at main.
    mem.ensure_shared().unwrap();
    let skill = SkillRef {
        scope: SkillScope::Shared,
        name: SkillName(x100_memory::EXECUTIVE_SUMMARY.into()),
    };
    assert!(mem.skill_on(&skill, &branch()).is_ok());
}

#[test]
fn a_compaction_writes_the_kept_notes_with_the_removed_ones_sources_and_deletes_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    three_notes(&mem);
    let merged = "Deploys go through `make ship` to box-7, never by hand.";
    let changes = mem
        .compaction(
            agent(),
            &[
                (note("deploys"), merged.into()),
                (note("lunch"), "The builder eats at noon.".into()),
            ],
        )
        .unwrap();
    assert_eq!(
        changes,
        vec![
            NoteChange::Delete(note("deploy-host.md")),
            NoteChange::Write {
                name: note("deploys.md"),
                body: merged.into(),
                sources: vec![Seq(10), Seq(11)],
            },
        ],
        "the unchanged note is not written again"
    );
    let before = mem.size_on(agent(), &Target::Main).unwrap();
    mem.change_notes(agent(), &branch(), &changes, at(2000), "compact")
        .unwrap();
    let after = mem.size_on(agent(), &branch()).unwrap();
    assert_eq!((before.notes, after.notes), (3, 2));
    assert!(after.bytes < before.bytes, "{before:?} {after:?}");
    assert!(after.index_fits());

    assert!(matches!(
        mem.compaction(agent(), &[]),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        mem.compaction(
            agent(),
            &[(note("a"), "1".into()), (note("a.md"), "2".into())]
        ),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn a_full_index_is_measured_as_over_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    let changes: Vec<NoteChange> = (0..200)
        .map(|i| NoteChange::Write {
            name: note(&format!("topic-{i:04}")),
            body: format!("Topic {i:04} says something long enough to take room in the index."),
            sources: vec![],
        })
        .collect();
    mem.change_notes(agent(), &Target::Main, &changes, at(1), "many")
        .unwrap();
    let size = mem.size_on(agent(), &Target::Main).unwrap();
    assert_eq!(size.notes, 200);
    assert!(size.unlisted > 0 && !size.index_fits(), "{size:?}");
}

#[test]
fn a_compaction_merges_around_a_note_written_on_main_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    three_notes(&mem);
    let changes = mem
        .compaction(
            agent(),
            &[(note("deploys"), "Ship with `make ship` to box-7.".into())],
        )
        .unwrap();
    mem.change_notes(agent(), &branch(), &changes, at(2000), "compact")
        .unwrap();
    // The agent writes a note on main while the compaction is on trial; both change the index.
    mem.remember(
        agent(),
        &note("new"),
        "Written meanwhile.",
        &[Seq(50)],
        at(3000),
    )
    .unwrap();
    mem.merge_notes(agent(), "trial/t1", at(4000))
        .expect("the index is remade, so it does not conflict");
    let mut names: Vec<String> = mem
        .notes(agent())
        .unwrap()
        .into_iter()
        .map(|n| n.name.0)
        .collect();
    names.sort();
    assert_eq!(names, ["deploys.md", "new.md"]);
    let index = mem.index(agent()).unwrap();
    assert!(
        index.contains("new.md") && !index.contains("lunch.md"),
        "{index}"
    );
    assert!(
        mem.branches(SkillScope::Agent(agent())).unwrap().is_empty(),
        "the branch is gone"
    );
}

#[test]
fn a_note_changed_on_both_sides_is_a_conflict_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    three_notes(&mem);
    let changes = mem
        .compaction(agent(), &[(note("deploys"), "Ship it.".into())])
        .unwrap();
    mem.change_notes(agent(), &branch(), &changes, at(2000), "compact")
        .unwrap();
    mem.remember(
        agent(),
        &note("lunch"),
        "Lunch moved to two.",
        &[Seq(60)],
        at(3000),
    )
    .unwrap();
    let main = mem.notes(agent()).unwrap();
    assert!(matches!(
        mem.merge_notes(agent(), "trial/t1", at(4000)),
        Err(Error::Conflict { .. })
    ));
    assert_eq!(mem.notes(agent()).unwrap(), main);
    assert_eq!(
        mem.branches(SkillScope::Agent(agent())).unwrap(),
        ["trial/t1"]
    );
}
