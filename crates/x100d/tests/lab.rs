//! M14's checks through the daemon as a child process, on the fake harness: Reflect now, the
//! project's "trial before commit" setting, and the lab.
//!
//! - Setting off: reflection's skill edit and compaction are committed directly, each a commit and
//!   an event, and no lab driver starts. A note that contradicts the log is corrected with no
//!   trial, here and with the setting on.
//! - Setting on: a skill change that helps is committed and one that does not is discarded, each
//!   after both arms ran three times, and both trials are in the log with their scores; the next
//!   driver loads the committed skill; the project's worktree, files and non-lab ledger lines are
//!   byte for byte what they were; a proposed team change is held with its telemetry.
//! - A compaction is committed only when every probe question is still answered, and discarded
//!   when it leaves the memory larger or the index over its limit.
//! - A trial that would pass the lab's cap is stopped and recorded as such.
//!
//! What a lab driver may reach is `lab_isolation.rs`.

mod common;
#[path = "lab/rig.rs"]
mod rig;

use common::WAIT;
use rig::*;
use x100_core::action::{ActionState, Request};
use x100_core::driver::{DriverState, EndReason, Purpose, StopReason};
use x100_core::event::ProjectEvent;
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope};
use x100_core::money::Money;
use x100_core::tool::{Proposal, ToolCall};
use x100_core::trial::{Arm, TrialChange, TrialState};
use x100_memory::{Memory, Target};

fn shared(name: &str) -> SkillRef {
    SkillRef {
        scope: SkillScope::Shared,
        name: SkillName(name.into()),
    }
}

/// `propose_change` of a new shared skill, tried on the brief with a check.
fn new_skill(name: &str, line: &str) -> String {
    format!(
        r#"  {{ call = {{ propose_change = {{ trial = {{ change = {{ new_skill = {{ skill = {{ scope = "shared", name = "{name}" }}, body = "{}" }} }}, eval = {{ items = [ {{ past_task = {{ task = "{{{{input:Accepted}}}}", checkpoint = "" }} }}, {{ check = {{ script = {{ command = "grep -q hello greeting.txt" }} }} }} ], runs_per_arm = 3 }} }} }} }}, save = "{name}" }},"#,
        skill_body(name, line)
    )
}

/// `propose_change` of an edit of a shared skill, tried on the brief.
fn edit_skill(name: &str, line: &str) -> String {
    format!(
        r#"  {{ call = {{ propose_change = {{ trial = {{ change = {{ edit_skill = {{ skill = {{ scope = "shared", name = "{name}" }}, body = "{}" }} }}, eval = {{ items = [ {{ past_task = {{ task = "{{{{input:Accepted}}}}", checkpoint = "" }} }} ], runs_per_arm = 3 }} }} }} }}, save = "{name}" }},"#,
        skill_body(name, line)
    )
}

/// `propose_change` of a compaction of the CEO's memory into `notes`, tried with one probe.
fn compaction(notes: &[(&str, &str)], save: &str) -> String {
    let notes: Vec<String> = notes
        .iter()
        .map(|(n, b)| format!(r#"["{n}", "{b}"]"#))
        .collect();
    format!(
        r#"  {{ call = {{ propose_change = {{ trial = {{ change = {{ compact_memory = {{ agent = "{{{{pack:Your id}}}}", notes = [{}] }} }}, eval = {{ items = [ {{ probe = {{ question = "How do deploys go?", answer = "by hand" }} }} ], runs_per_arm = 3 }} }} }} }}, save = "{save}" }},"#,
        notes.join(", ")
    )
}

const CORRECTION: &str = r#"  { call = { propose_change = { correction = { agent = "{{pack:Your id}}", note = "deploys", body = "Deploys go through `make ship`, never by hand.", sources = [1] } } }, save = "corrected" },"#;

#[tokio::test(flavor = "multi_thread")]
async fn with_the_setting_off_reflection_commits_directly_and_no_lab_driver_starts() {
    let reflection = [
        CORRECTION.to_owned(),
        edit_skill("executive-summary", "Lead with the answer, in one line."),
        compaction(
            &[
                ("deploys", "Deploys go through `make ship`, never by hand."),
                ("facts", "Host box-7. Lunch at noon."),
            ],
            "compacted",
        ),
    ]
    .join("\n");
    let r = Rig::start("off", &script(&reflection, ""), &[]).await;
    assert!(
        !r.state().project().unwrap().settings.trial_before_commit,
        "trial before commit is off by default"
    );
    let memory = Memory::new(&r.dir);
    let skills_before = git(
        &r.dir.join(".x100/skills"),
        &["rev-list", "--count", "main"],
    );
    r.reflect().await;

    let events = r.events().await;
    assert!(
        !events.iter().any(|e| matches!(
            e.event,
            ProjectEvent::TrialProposed(_) | ProjectEvent::Trial { .. }
        )),
        "nothing went on trial"
    );
    assert!(r.lab_drivers().is_empty(), "no lab driver started");

    // The correction: the note says what the log says, one commit on the CEO's main branch.
    let corrected = events
        .iter()
        .rev()
        .find_map(|e| match &e.event {
            ProjectEvent::MemoryWritten {
                agent,
                note,
                commit,
            } if *agent == r.ceo && note.0 == "deploys.md" => Some(commit.clone()),
            _ => None,
        })
        .expect("the correction is an event");
    let agent_repo = r.dir.join(".x100/agents").join(r.ceo.to_string());
    assert_eq!(
        git(&agent_repo, &["cat-file", "-t", &corrected.0]),
        "commit"
    );

    // The skill edit is on main as one commit, its version raised, with no trial behind it.
    let summary = shared(x100_memory::EXECUTIVE_SUMMARY);
    let (skill_commit, version, trial) = events
        .iter()
        .rev()
        .find_map(|e| match &e.event {
            ProjectEvent::SkillCommitted {
                skill,
                version,
                commit,
                trial,
            } if *skill == summary => Some((commit.clone(), *version, *trial)),
            _ => None,
        })
        .expect("the skill change is an event");
    assert_eq!(trial, None);
    assert_eq!(version.0, 2, "the built-in was version 1");
    let skills_repo = r.dir.join(".x100/skills");
    assert_eq!(git(&skills_repo, &["rev-parse", "main"]), skill_commit.0);
    assert_eq!(
        git(&skills_repo, &["rev-list", "--count", "main"])
            .parse::<u32>()
            .unwrap(),
        skills_before.parse::<u32>().unwrap() + 1
    );
    let text = memory.skill(&summary, &Target::Main).unwrap().text;
    assert!(
        text.contains("Lead with the answer, in one line."),
        "{text}"
    );

    // The compaction is one commit too, and the memory holds exactly what it named.
    let (compact_commit, trial) = events
        .iter()
        .find_map(|e| match &e.event {
            ProjectEvent::MemoryCompacted {
                agent,
                commit,
                trial,
            } if *agent == r.ceo => Some((commit.clone(), *trial)),
            _ => None,
        })
        .expect("the compaction is an event");
    assert_eq!(trial, None);
    assert_eq!(git(&agent_repo, &["rev-parse", "main"]), compact_commit.0);
    let mut names: Vec<String> = memory
        .notes(r.ceo)
        .unwrap()
        .into_iter()
        .map(|n| n.name.0)
        .collect();
    names.sort();
    assert_eq!(names, ["deploys.md", "facts.md"]);
    assert!(
        memory
            .branches(SkillScope::Agent(r.ceo))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_setting_on_a_helpful_skill_is_committed_and_an_unhelpful_one_dropped() {
    let team = r#"  { call = { propose_change = { team = { changes = [ { add_lead = { role = "Ops", charter = "Owns deploys.", effort = { min = "low", max = "high" }, budget = 1000000 } } ], telemetry = "TELEMETRY: deploys failed 4 of 6 times (events 12 and 19), and nobody owns them.", team_toml = "[leads]" } } }, save = "team" },"#;
    let reflection = [
        CORRECTION.to_owned(),
        new_skill("greeting", "GREETING-HELPS: write hello into greeting.txt."),
        new_skill("noise", "Say something kind first."),
        team.to_owned(),
    ]
    .join("\n");
    let r = Rig::start("on", &script(&reflection, ""), &[]).await;
    r.set_trials(true).await;

    // What no trial may touch, as it stands before reflecting.
    let before_events = r.events().await;
    let worktree = files(&r.dir);
    let project_refs = refs(&r.dir);
    let store = x100_store::project_dir(&r.env.data.join("projects"), r.project);
    let store_files = files(&store.join("files"));
    let work_ledger = ledger(&before_events, false);
    assert!(!work_ledger.is_empty(), "the brief's work is on the ledger");

    r.reflect().await;
    let events = r.events().await;
    let state = r.state();

    // The correction went straight in, with the setting on too.
    let corrections: Vec<_> = events
        .iter()
        .filter(|e| matches!(&e.event, ProjectEvent::MemoryWritten { note, .. } if note.0 == "deploys.md"))
        .collect();
    assert_eq!(
        corrections.len(),
        2,
        "the brief's note, then its correction"
    );
    assert_eq!(
        Memory::new(&r.dir)
            .note(r.ceo, &NoteName("deploys".into()))
            .unwrap()
            .body,
        "Deploys go through `make ship`, never by hand."
    );

    // Two trials, both in the log with every run's score.
    let trials = r.client.list_trials(r.project).await.unwrap().trials;
    assert_eq!(trials.len(), 2, "{trials:#?}");
    let about = |name: &str| {
        trials
            .iter()
            .find(|t| matches!(&t.trial.change, TrialChange::NewSkill { skill, .. } if skill.name.0 == name))
            .unwrap()
    };
    let (helpful, unhelpful) = (about("greeting"), about("noise"));
    for t in [helpful, unhelpful] {
        for arm in [Arm::Baseline, Arm::Changed] {
            let runs: Vec<u8> = t
                .runs
                .iter()
                .filter(|s| s.arm == arm)
                .map(|s| s.run)
                .collect();
            assert_eq!(runs, [1, 2, 3], "three runs of each arm: {t:#?}");
        }
        for s in &t.runs {
            assert_eq!(s.checks_total, 2, "reported, and the script");
            assert_eq!(s.cost, Money::from_micros(2000));
        }
    }
    let passed = |t: &x100_core::fold::TrialRecord, arm| -> u32 {
        t.runs
            .iter()
            .filter(|s| s.arm == arm)
            .map(|s| s.checks_passed)
            .sum()
    };
    assert_eq!(
        (
            passed(helpful, Arm::Baseline),
            passed(helpful, Arm::Changed)
        ),
        (3, 6)
    );
    assert_eq!(
        (
            passed(unhelpful, Arm::Baseline),
            passed(unhelpful, Arm::Changed)
        ),
        (3, 3)
    );
    assert_eq!(helpful.state, TrialState::Committed);
    assert_eq!(unhelpful.state, TrialState::Discarded);
    assert!(
        unhelpful
            .why
            .as_deref()
            .is_some_and(|w| w.contains("no better")),
        "{:?}",
        unhelpful.why
    );
    let committed = events.iter().find_map(|e| match &e.event {
        ProjectEvent::SkillCommitted {
            skill,
            trial,
            commit,
            ..
        } if *skill == shared("greeting") => Some((*trial, commit.clone())),
        _ => None,
    });
    assert_eq!(
        committed.as_ref().map(|c| c.0),
        Some(Some(helpful.trial.id))
    );
    assert_eq!(helpful.commit.as_ref(), committed.as_ref().map(|c| &c.1));
    let memory = Memory::new(&r.dir);
    assert!(
        memory
            .skill(&shared("greeting"), &Target::Main)
            .unwrap()
            .text
            .contains("GREETING-HELPS")
    );
    assert!(
        memory.skill(&shared("noise"), &Target::Main).is_err(),
        "dropped"
    );
    assert!(
        memory.branches(SkillScope::Shared).unwrap().is_empty(),
        "both branches are gone"
    );

    // Twelve lab drivers, each ended as a lab run, the changed arm's pack carrying the skill.
    let lab = r.lab_drivers();
    assert_eq!(lab.len(), 12);
    for d in &lab {
        assert_eq!(
            d.state,
            DriverState::Ended(EndReason::Stopped(StopReason::LabRunEnded))
        );
        let Purpose::Lab { trial, arm, .. } = d.driver.purpose else {
            unreachable!()
        };
        let pack = r.pack(d.driver.id).await;
        assert!(pack.starts_with("LAB RUN"), "{pack}");
        if trial == helpful.trial.id {
            assert_eq!(
                pack.contains("GREETING-HELPS"),
                arm == Arm::Changed,
                "{pack}"
            );
        }
    }

    // Nothing the trials did touched the project.
    assert_eq!(
        files(&r.dir),
        worktree,
        "the worktree and the project's files"
    );
    assert_eq!(
        refs(&r.dir),
        project_refs,
        "the project repository's branches"
    );
    assert_eq!(
        files(&store.join("files")),
        store_files,
        "the project's file store"
    );
    assert_eq!(
        ledger(&events, false),
        work_ledger,
        "the ledger's lines but the lab's"
    );
    assert_eq!(
        ledger(&events, true).len(),
        12,
        "every lab run is on the lab's line"
    );
    assert_eq!(
        std::fs::read_to_string(r.dir.join("greeting.txt")).unwrap(),
        "hello",
        "main holds the brief's own work, not a lab run's"
    );

    // The proposed team change waits for the builder, with its telemetry.
    let held = state
        .actions()
        .find(|a| {
            matches!(
                &a.action.request,
                Request::Tool {
                    call: ToolCall::ProposeChange(Proposal::Team { .. }),
                    ..
                }
            )
        })
        .expect("the team change is an action");
    assert_eq!(held.state, ActionState::Held);
    let Request::Tool {
        call: ToolCall::ProposeChange(Proposal::Team { telemetry, .. }),
        ..
    } = &held.action.request
    else {
        unreachable!()
    };
    assert!(
        telemetry.starts_with("TELEMETRY: deploys failed 4 of 6"),
        "{telemetry}"
    );
    let shown = x100_gate::Shown::of(&state, held).expect("a held action is shown");
    assert!(
        shown.what.contains("TELEMETRY: deploys failed"),
        "{}",
        shown.what
    );
    r.client
        .decide_action(
            r.project,
            held.action.id,
            x100_core::action::Decision::Approved,
        )
        .await
        .unwrap();
    r.client
        .wait_for(WAIT, |s| {
            s.projects
                .get(&r.project)?
                .agents()
                .find(|a| a.role == "Ops")
                .map(|_| ())
        })
        .await
        .expect("approved, the lead joins");

    // The next driver the CEO starts loads the committed skill and not the dropped one.
    r.idle().await;
    let live = r
        .state()
        .agent(r.ceo)
        .unwrap()
        .drivers
        .iter()
        .copied()
        .find(|d| {
            r.state()
                .driver(*d)
                .is_some_and(|d| d.state.is_live() && d.driver.purpose == Purpose::Work)
        });
    if let Some(old) = live {
        r.client
            .request(x100_proto::StopDriver {
                project: r.project,
                driver: old,
            })
            .await
            .unwrap();
    }
    let count = r.state().agent(r.ceo).unwrap().drivers.len();
    r.client
        .send_chat(r.project, "hello again", vec![], None)
        .await
        .unwrap();
    let new = r
        .client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&r.project)?;
            let a = p.agent(r.ceo)?;
            a.drivers[count..].iter().copied().find(|d| {
                p.driver(*d)
                    .is_some_and(|d| d.driver.purpose == Purpose::Work)
            })
        })
        .await
        .expect("a new work driver");
    let pack = loop {
        if let Ok(p) = r
            .client
            .request(x100_proto::ReadStartPack {
                project: r.project,
                driver: new,
            })
            .await
        {
            break p.text;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let skills = pack
        .split("SKILLS YOU CAN LOAD")
        .nth(1)
        .and_then(|rest| rest.split("\n\n").next())
        .expect("the pack lists skills");
    assert!(
        skills.contains("greeting") && !skills.contains("noise"),
        "{skills}"
    );
}

/// A note name long enough that only a few fit in the index.
fn topic(i: usize) -> String {
    format!("topic-number-{i:04}-with-a-name-long-enough-to-fill-the-index")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_compaction_is_committed_only_when_it_is_smaller_fits_the_index_and_answers_every_probe()
{
    let big = "x".repeat(30 * 1024);
    let over: Vec<(String, String)> = (0..30)
        .map(|i| (topic(i), format!("Topic {i} in short.")))
        .collect();
    let over: Vec<(&str, &str)> = over.iter().map(|(n, b)| (n.as_str(), b.as_str())).collect();
    let reflection = [
        // Larger than the memory it replaces.
        compaction(
            &[("deploys", "Deploys go by hand."), ("everything", &big)],
            "larger",
        ),
        // Smaller, but the probe's answer is gone.
        compaction(&[("ops", "Host box-7. Lunch at noon.")], "forgets"),
        // Smaller, but more notes than the index can list.
        compaction(&over, "overflows"),
        // Smaller, every note indexed, and every probe still answered.
        compaction(
            &[
                ("deploys", "Deploys go by hand."),
                ("facts", "Host box-7. Lunch at noon."),
            ],
            "good",
        ),
    ]
    .join("\n");
    let r = Rig::start("compact", &script(&reflection, ""), &[]).await;
    r.set_trials(true).await;
    // The CEO has remembered a great deal: forty long notes besides the brief's three.
    let memory = Memory::new(&r.dir);
    let many: Vec<x100_memory::NoteChange> = (0..40)
        .map(|i| x100_memory::NoteChange::Write {
            name: NoteName(topic(i)),
            body: format!(
                "Topic {i}: {}",
                "a long account of what was found. ".repeat(16)
            ),
            sources: vec![x100_core::event::Seq(1)],
        })
        .collect();
    memory
        .change_notes(
            r.ceo,
            &Target::Main,
            &many,
            x100_core::time::Timestamp::from_millis(1),
            "many",
        )
        .unwrap();
    let before = memory.size_on(r.ceo, &Target::Main).unwrap();
    assert!(before.index_fits() || before.unlisted > 0);

    r.reflect().await;
    let trials = r.client.list_trials(r.project).await.unwrap().trials;
    assert_eq!(trials.len(), 4, "{trials:#?}");
    let of = |first: &str| {
        trials
            .iter()
            .find(|t| matches!(&t.trial.change, TrialChange::CompactMemory { notes, .. } if notes[0].0 .0 == first))
            .unwrap_or_else(|| panic!("the trial whose first note is {first}"))
    };
    let (larger, forgets, overflows) = (
        trials.iter().find(|t| matches!(&t.trial.change, TrialChange::CompactMemory { notes, .. } if notes.len() == 2 && notes[1].0 .0 == "everything")).unwrap(),
        of("ops"),
        of(&topic(0)),
    );
    let good = trials
        .iter()
        .find(|t| matches!(&t.trial.change, TrialChange::CompactMemory { notes, .. } if notes.len() == 2 && notes[1].0 .0 == "facts"))
        .unwrap();

    for (t, why) in [
        (larger, "must make it smaller"),
        (overflows, "over its limit"),
    ] {
        assert_eq!(t.state, TrialState::Discarded);
        assert!(t.runs.is_empty(), "decided without a run");
        assert!(
            t.why.as_deref().is_some_and(|w| w.contains(why)),
            "{:?}",
            t.why
        );
    }
    assert_eq!(forgets.state, TrialState::Discarded);
    assert_eq!(forgets.runs.len(), 6);
    assert!(
        forgets
            .runs
            .iter()
            .filter(|s| s.arm == Arm::Changed)
            .all(|s| s.checks_passed == 0),
        "without the note, the probe goes unanswered: {forgets:#?}"
    );
    assert!(
        forgets
            .runs
            .iter()
            .filter(|s| s.arm == Arm::Baseline)
            .all(|s| s.checks_passed == 1)
    );
    assert!(
        forgets
            .why
            .as_deref()
            .is_some_and(|w| w.contains("unanswered")),
        "{:?}",
        forgets.why
    );
    assert_eq!(good.state, TrialState::Committed, "{good:#?}");
    assert_eq!(good.runs.len(), 6);
    assert!(
        good.runs
            .iter()
            .all(|s| s.checks_passed == s.checks_total && s.checks_total == 1)
    );

    let events = r.events().await;
    let compacted: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.event {
            ProjectEvent::MemoryCompacted {
                agent,
                trial,
                commit,
            } if *agent == r.ceo => Some((*trial, commit.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(compacted.len(), 1, "only the good compaction reached main");
    assert_eq!(compacted[0].0, Some(good.trial.id));
    let mut names: Vec<String> = memory
        .notes(r.ceo)
        .unwrap()
        .into_iter()
        .map(|n| n.name.0)
        .collect();
    names.sort();
    assert_eq!(names, ["deploys.md", "facts.md"]);
    let after = memory.size_on(r.ceo, &Target::Main).unwrap();
    assert!(
        after.bytes < before.bytes && after.index_fits(),
        "{before:?} {after:?}"
    );
    assert!(
        memory
            .branches(SkillScope::Agent(r.ceo))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trial_that_would_pass_the_labs_cap_is_stopped_and_recorded_as_such() {
    let reflection = new_skill("greeting", "GREETING-HELPS: write hello into greeting.txt.");
    let r = Rig::start("cap", &script(&reflection, ""), &[]).await;
    r.set_trials(true).await;
    // One dollar for the project, one percent of it for the lab: ten thousand micro-dollars, and
    // each lab run costs two thousand.
    for change in [
        x100_core::project::SettingChange::Budget(Money::from_dollars(1)),
        x100_core::project::SettingChange::LabShare(x100_core::money::Percent::new(1).unwrap()),
    ] {
        r.client.change_setting(r.project, change).await.unwrap();
    }
    r.client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&r.project)?.project()?;
            (p.settings.lab_cap() == Money::from_micros(10_000)).then_some(())
        })
        .await
        .unwrap();

    r.reflect().await;
    let trials = r.client.list_trials(r.project).await.unwrap().trials;
    assert_eq!(trials.len(), 1);
    let t = &trials[0];
    assert_eq!(t.state, TrialState::Stopped, "{t:#?}");
    assert!(
        t.why
            .as_deref()
            .is_some_and(|w| w.contains("would pass the lab's cap")),
        "{:?}",
        t.why
    );
    // Five runs of two thousand each reach the cap exactly; a sixth would have passed it, so it
    // never started.
    assert_eq!(t.runs.len(), 5, "{:#?}", t.runs);
    assert_eq!(
        t.runs.last().map(|s| (s.arm, s.run)),
        Some((Arm::Baseline, 3))
    );
    let state = r.state();
    let lab = state.budget().unwrap().lab;
    assert_eq!(
        lab.spent,
        Money::from_micros(10_000),
        "the cap was reached and never passed"
    );
    assert_eq!(
        r.lab_drivers().len(),
        5,
        "no lab driver started for the sixth run"
    );
    let memory = Memory::new(&r.dir);
    assert!(
        memory.skill(&shared("greeting"), &Target::Main).is_err(),
        "nothing was committed"
    );
    assert!(memory.branches(SkillScope::Shared).unwrap().is_empty());
    let events = r.events().await;
    assert!(events.iter().any(|e| matches!(
        &e.event,
        ProjectEvent::Trial {
            event: x100_core::trial::TrialEvent::Stopped { .. },
            ..
        }
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trial_cut_off_by_the_daemon_stopping_is_stopped_and_its_branch_deleted() {
    // A lab run that is still going when the daemon stops.
    let slow = r#"
[[turn]]
when = { start_pack_contains = "LAB RUN", input_contains = "Write the greeting" }
repeat = true
steps = [{ text = "thinking" }, { sleep_ms = 120000 }]
"#;
    let reflection = new_skill("greeting", "GREETING-HELPS: write hello into greeting.txt.");
    let mut r = Rig::start("cut", &script(&reflection, slow), &[]).await;
    r.set_trials(true).await;
    r.client.reflect(r.project).await.unwrap();
    let running = r
        .client
        .wait_for(rig::LONG, |s| {
            let p = s.projects.get(&r.project)?;
            p.drivers()
                .find(|d| d.driver.purpose != Purpose::Work && d.state == DriverState::Running)
                .map(|d| d.driver.id)
        })
        .await
        .expect("a lab run is under way");
    let trial = r.state().trials().next().unwrap().trial.clone();
    assert_eq!(
        Memory::new(&r.dir).branches(SkillScope::Shared).unwrap(),
        std::slice::from_ref(&trial.branch)
    );

    // The processes of the drivers live now, the lab run's among them.
    let before: Vec<u32> = r.env.recorded_pids().into_iter().map(|(_, p)| p).collect();
    assert!(!before.is_empty());
    r.restart().await;
    let state = r
        .client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&r.project)?;
            (p.trial(trial.id)?.state == TrialState::Stopped).then(|| p.clone())
        })
        .await
        .expect("the trial is stopped");
    let t = state.trial(trial.id).unwrap();
    assert!(
        t.why
            .as_deref()
            .is_some_and(|w| w.contains("daemon stopped")),
        "{:?}",
        t.why
    );
    assert!(
        Memory::new(&r.dir)
            .branches(SkillScope::Shared)
            .unwrap()
            .is_empty(),
        "its branch is deleted"
    );
    assert!(
        Memory::new(&r.dir)
            .skill(&shared("greeting"), &Target::Main)
            .is_err()
    );
    assert!(!state.driver(running).unwrap().state.is_live());
    // The call that proposed it ended, and the CEO was told it was not carried out.
    let proposal = state
        .actions()
        .find(|a| {
            matches!(
                &a.action.request,
                Request::Tool {
                    call: ToolCall::ProposeChange(Proposal::Trial { .. }),
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(proposal.state, ActionState::Failed, "{proposal:#?}");
    for pid in before {
        assert!(
            !common::running(pid),
            "a process {pid} of the stopped daemon's drivers was left running"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn where_no_script_can_decide_a_judging_lab_driver_marks_each_run_against_the_rubric() {
    // The judge reads the worker's summary and marks it.
    let judge = r#"
[[turn]]
when = { start_pack_contains = "LAB RUN", input_contains = "wrote hello" }
repeat = true
steps = [
  { usage = { input = 10, output = 5, cost_micros = 500 } },
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "90: a proper greeting" } } },
]

[[turn]]
when = { start_pack_contains = "LAB RUN", input_contains = "JUDGE" }
repeat = true
steps = [
  { usage = { input = 10, output = 5, cost_micros = 500 } },
  { call = { report = { task = "{{input:Task}}", outcome = "completed", summary = "30: barely a greeting" } } },
]
"#;
    let reflection = format!(
        r#"  {{ call = {{ propose_change = {{ trial = {{ change = {{ new_skill = {{ skill = {{ scope = "shared", name = "greeting" }}, body = "{}" }} }}, eval = {{ items = [ {{ past_task = {{ task = "{{{{input:Accepted}}}}", checkpoint = "" }} }}, {{ check = {{ rubric = {{ text = "Does it greet the reader warmly?" }} }} }} ], runs_per_arm = 3 }} }} }} }}, save = "t" }},"#,
        skill_body("greeting", "GREETING-HELPS: write hello into greeting.txt.")
    );
    let r = Rig::start("judged", &script(&reflection, judge), &[]).await;
    r.set_trials(true).await;
    r.reflect().await;
    let trials = r.client.list_trials(r.project).await.unwrap().trials;
    let t = &trials[0];
    assert_eq!(t.runs.len(), 6);
    for s in &t.runs {
        let want = match s.arm {
            Arm::Changed => 90,
            Arm::Baseline => 30,
        };
        assert_eq!(s.judged, Some(want), "{s:?}");
        assert_eq!(
            (s.checks_passed, s.checks_total),
            (1, 1),
            "reported, and no script"
        );
        assert_eq!(
            s.cost,
            Money::from_micros(2_500),
            "the judge is on the run's cost"
        );
    }
    assert_eq!(t.state, TrialState::Committed, "{t:#?}");
    // A worker and a judge for each of the six runs, all lab drivers.
    assert_eq!(r.lab_drivers().len(), 12);
}
