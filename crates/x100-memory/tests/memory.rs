//! The repositories against real git in a temporary directory.

use std::process::Command;

use x100_core::event::Seq;
use x100_core::id::AgentId;
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope};
use x100_core::time::Timestamp;
use x100_memory::{INDEX_LIMIT, Memory, NoteChange, SKILLS_LIMIT, Target};

fn agent() -> AgentId {
    AgentId::from_u128(7)
}

fn note(name: &str) -> NoteName {
    NoteName(name.into())
}

fn at(ms: u64) -> Timestamp {
    Timestamp::from_millis(ms)
}

fn shared(name: &str) -> SkillRef {
    SkillRef {
        scope: SkillScope::Shared,
        name: SkillName(name.into()),
    }
}

fn skill_text(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n")
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}

fn agent_dir(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".x100").join("agents").join(agent().to_string())
}

#[test]
fn a_note_round_trips_through_the_files_as_one_commit_with_its_sources() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    let (name, commit) = mem
        .remember(
            agent(),
            &note("Deploys"),
            "Deploys go through `make ship`.",
            &[Seq(12), Seq(40)],
            at(1000),
        )
        .expect("the note is written");
    assert_eq!(name.0, "deploys.md");

    let read = mem.note(agent(), &note("deploys")).unwrap();
    assert_eq!(read.body, "Deploys go through `make ship`.");
    assert_eq!(read.sources, vec![Seq(12), Seq(40)]);
    assert_eq!(read.written, at(1000));
    assert_eq!(read.commit, commit);

    // Plain Markdown on disk, with the sources in the front matter, and the index beside it.
    let file = agent_dir(dir.path()).join("memory/deploys.md");
    let text = std::fs::read_to_string(file).unwrap();
    assert!(
        text.starts_with("---\nwritten: 1000\nsources: [12, 40]\n---\n"),
        "{text}"
    );
    let index = mem.index(agent()).unwrap();
    assert!(
        index.contains("- [deploys.md](deploys.md): Deploys go through `make ship`."),
        "{index}"
    );

    // The checkout is clean and the change is a commit, which is the one returned.
    let repo = agent_dir(dir.path());
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), commit.0);
    assert_eq!(
        git(&repo, &["log", "--format=%s"]),
        "remember deploys.md\nx100: start"
    );
}

#[test]
fn a_note_is_replaced_by_the_same_name_and_saying_it_again_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    mem.remember(agent(), &note("stack"), "Rust.", &[], at(1))
        .unwrap();
    mem.remember(agent(), &note("stack"), "Rust and SQLite.", &[], at(2))
        .unwrap();
    assert_eq!(
        mem.note(agent(), &note("stack")).unwrap().body,
        "Rust and SQLite."
    );
    assert_eq!(mem.notes(agent()).unwrap().len(), 1);
    let again = mem.remember(agent(), &note("stack"), "Rust and SQLite.", &[], at(2));
    assert!(
        again.is_err(),
        "the same text at the same time is no change"
    );
}

#[test]
fn names_and_sizes_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    for bad in ["../x", "a/b", "INDEX", "", "Has Space", "-dash"] {
        assert!(
            mem.remember(agent(), &note(bad), "b", &[], at(1)).is_err(),
            "{bad:?}"
        );
    }
    let big = "x".repeat(x100_memory::NOTE_LIMIT + 1);
    assert!(
        mem.remember(agent(), &note("big"), &big, &[], at(1))
            .is_err()
    );
    assert!(
        mem.remember(agent(), &note("empty"), "  ", &[], at(1))
            .is_err()
    );
    assert!(mem.note(agent(), &note("missing")).is_err());
}

#[test]
fn the_index_stays_bounded_however_many_notes_there_are_and_search_finds_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    // One commit with a thousand notes, as a compaction would write them.
    let changes: Vec<NoteChange> = (0..1000)
        .map(|i| NoteChange::Write {
            name: note(&format!("topic-{i:04}")),
            body: format!(
                "Topic {i}: a sentence about it that makes the line long enough to count."
            ),
            sources: vec![Seq(i)],
        })
        .collect();
    mem.change_notes(
        agent(),
        &Target::Main,
        &changes,
        at(5000),
        "a thousand notes",
    )
    .unwrap()
    .expect("committed");
    let index = mem.index(agent()).unwrap();
    assert!(index.len() <= INDEX_LIMIT, "{} bytes", index.len());
    assert!(index.contains(" more notes, not listed here"), "{index}");
    assert_eq!(mem.notes(agent()).unwrap().len(), 1000);

    // A note the index leaves out is found by what it says.
    let hits = mem.search_notes(agent(), "Topic 7:", 5).unwrap();
    assert_eq!(
        hits.first().map(|h| h.name.0.as_str()),
        Some("topic-0007.md")
    );
    assert!(
        mem.search_notes(agent(), "no such words", 5)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_change_on_a_trial_branch_is_not_on_main_until_it_is_merged() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    let seeded = mem.ensure_shared().unwrap();
    assert_eq!(seeded.len(), 1, "the built-in skill");
    assert_eq!(seeded[0].skill, shared("executive-summary"));
    assert!(mem.ensure_shared().unwrap().is_empty(), "seeded once");

    let (v1, c1) = mem
        .write_skill(
            &shared("deploy"),
            &skill_text("deploy", "How to deploy.", "Run make ship."),
            &Target::Main,
            "add deploy",
        )
        .unwrap()
        .unwrap();
    assert_eq!(v1.0, 1);
    let on_main = mem.skill(&shared("deploy"), &Target::Main).unwrap();
    assert_eq!(on_main.version.0, 1);
    assert!(on_main.text.contains("version: 1"));

    let branch = Target::Branch("trial/deploy-edit".into());
    let (v2, c2) = mem
        .write_skill(
            &shared("deploy"),
            &skill_text("deploy", "How to deploy.", "Run make ship, then check."),
            &branch,
            "edit deploy",
        )
        .unwrap()
        .unwrap();
    assert_eq!(v2.0, 2, "carries the version it will have when merged");
    assert_ne!(c1, c2);
    // Main, and the checkout the builder reads, are as they were.
    assert!(
        !mem.skill(&shared("deploy"), &Target::Main)
            .unwrap()
            .text
            .contains("then check")
    );
    let checked_out = dir.path().join(".x100/skills/deploy/SKILL.md");
    assert!(
        !std::fs::read_to_string(checked_out)
            .unwrap()
            .contains("then check")
    );
    assert_eq!(
        mem.branches(SkillScope::Shared).unwrap(),
        vec!["trial/deploy-edit"]
    );
    assert!(
        mem.skill(&shared("deploy"), &branch)
            .unwrap()
            .text
            .contains("then check")
    );

    let merged = mem.merge(SkillScope::Shared, "trial/deploy-edit").unwrap();
    let now = mem.skill(&shared("deploy"), &Target::Main).unwrap();
    assert_eq!(now.version.0, 2);
    assert!(now.text.contains("then check"));
    let checked_out = dir.path().join(".x100/skills/deploy/SKILL.md");
    assert!(
        std::fs::read_to_string(checked_out)
            .unwrap()
            .contains("then check"),
        "the checkout follows"
    );
    assert_eq!(
        git(&dir.path().join(".x100/skills"), &["rev-parse", "HEAD"]),
        merged.0
    );
    assert!(
        mem.branches(SkillScope::Shared).unwrap().is_empty(),
        "the branch is gone"
    );
}

#[test]
fn discarding_a_branch_drops_its_change_and_main_never_saw_it() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    let branch = Target::Branch("trial/new".into());
    mem.write_skill(
        &shared("fresh"),
        &skill_text("fresh", "A new one.", "Do it."),
        &branch,
        "new",
    )
    .unwrap();
    assert!(mem.skill(&shared("fresh"), &Target::Main).is_err());
    mem.discard(SkillScope::Shared, "trial/new").unwrap();
    assert!(mem.skill(&shared("fresh"), &branch).is_err());
    assert!(mem.discard(SkillScope::Shared, "trial/new").is_err());
    for bad in ["main", "../x", "trial/", "trial/a..b"] {
        assert!(mem.discard(SkillScope::Shared, bad).is_err(), "{bad}");
    }
}

#[test]
fn a_memory_compaction_can_wait_on_a_branch_too() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    mem.remember(agent(), &note("a"), "First.", &[], at(1))
        .unwrap();
    mem.remember(agent(), &note("b"), "Second.", &[], at(2))
        .unwrap();
    let branch = Target::Branch("trial/compact".into());
    let changes = [
        NoteChange::Write {
            name: note("ab"),
            body: "First and second.".into(),
            sources: vec![],
        },
        NoteChange::Delete(note("a")),
        NoteChange::Delete(note("b")),
    ];
    mem.change_notes(agent(), &branch, &changes, at(3), "compact")
        .unwrap()
        .unwrap();
    assert_eq!(mem.notes(agent()).unwrap().len(), 2, "main still has both");
    mem.merge(SkillScope::Agent(agent()), "trial/compact")
        .unwrap();
    let names: Vec<String> = mem
        .notes(agent())
        .unwrap()
        .into_iter()
        .map(|n| n.name.0)
        .collect();
    assert_eq!(names, vec!["ab.md"]);
    assert!(!mem.index(agent()).unwrap().contains("[a.md]"));
}

#[test]
fn a_skills_section_lists_skills_and_carries_only_the_asked_for_ones_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    mem.ensure_shared().unwrap();
    for i in 0..100 {
        let name = format!("skill-{i:03}");
        mem.write_skill(&shared(&name), &skill_text(&name, "A skill whose description runs on long enough that a hundred of them cannot all be listed in the space a start pack gives skills, which is the point of the cut.", "Body."), &Target::Main, "add")
            .unwrap();
    }
    let section = mem
        .skills_section(agent(), &["executive-summary"], &[])
        .unwrap();
    let hidden = mem
        .skills_section(agent(), &[], &["executive-summary"])
        .unwrap();
    assert!(!hidden.contains("executive-summary"));
    assert!(section.len() <= SKILLS_LIMIT, "{} bytes", section.len());
    assert!(
        section.starts_with("SKILL executive-summary"),
        "the asked-for skill is in full"
    );
    assert!(section.contains("SKILLS YOU CAN LOAD"));
    assert!(section.contains(" more skills, not listed here"));
    assert!(!section.contains("Body."), "listed skills carry no body");
    let alone = x100_memory::builtin_section(&["executive-summary"]);
    assert!(alone.starts_with("SKILL executive-summary"));
}

#[test]
fn the_builders_edits_in_the_checkout_do_not_reach_agents_until_committed() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    mem.remember(agent(), &note("rule"), "Original.", &[], at(1))
        .unwrap();
    let file = agent_dir(dir.path()).join("memory/rule.md");
    std::fs::write(&file, "---\nwritten: 1\nsources: []\n---\n\nHand edit.\n").unwrap();
    assert_eq!(mem.note(agent(), &note("rule")).unwrap().body, "Original.");
}

#[test]
fn a_lab_driver_can_be_given_the_pack_sections_as_a_trial_branch_would_make_them() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Memory::new(dir.path());
    mem.ensure_shared().unwrap();
    mem.write_skill(
        &shared("deploy"),
        &skill_text("deploy", "Original.", "Steps."),
        &Target::Main,
        "add",
    )
    .unwrap();
    let branch = Target::Branch("trial/changed".into());
    mem.write_skill(
        &shared("deploy"),
        &skill_text("deploy", "Changed.", "Steps."),
        &branch,
        "edit",
    )
    .unwrap();
    let main = mem.skills_section(agent(), &[], &[]).unwrap();
    let trial = mem.skills_section_on(agent(), &[], &[], &branch).unwrap();
    assert!(
        main.contains("Original.") && !main.contains("Changed."),
        "{main}"
    );
    assert!(
        trial.contains("Changed.") && !trial.contains("Original."),
        "{trial}"
    );
    // An agent repository with no such branch reads at main.
    mem.remember(agent(), &note("a"), "First.", &[], at(1))
        .unwrap();
    assert_eq!(
        mem.index_on(agent(), &branch).unwrap(),
        mem.index(agent()).unwrap()
    );
}
