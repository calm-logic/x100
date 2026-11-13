//! M13's check through the daemon as a child process, on the fake harness: what agents remember
//! and load round-trips through the files, a start pack carries the index and not the notes, the
//! log can be searched, a new driver gets the skills that are on main and not those on a trial
//! branch, and every change is a git commit and an event.
//!
//! The CEO is the agent. Its driver is stopped between steps so that the next message starts a
//! driver from a fresh start pack. Where a test needs an agent that has remembered a lot, or a
//! change on a trial branch, it uses `x100-memory`'s interface on the project's folder, as the
//! lab will.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use x100_client::Client;
use x100_core::event::{ProjectEvent, Seq};
use x100_core::id::{AgentId, DriverId, ProjectId};
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope};
use x100_core::message::Route;
use x100_core::time::Timestamp;
use x100_memory::{INDEX_LIMIT, Memory, NoteChange, SKILLS_LIMIT, Target};
use x100_proto::{LogId, LogPage, Page, ReadLog, ReadStartPack, StopDriver};

/// Each step is one message from the builder, named by a marker word. The CEO answers by telling
/// the builder what the tool returned, so the test reads the result as the agent did.
const SCRIPT: &str = r##"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

[[turn]]
when = { input_contains = "STEP-REMEMBER" }
repeat = true
steps = [
  { call = { remember = { note = "Deploys", body = "Deploys go through `make ship`, never by hand.\n\nThe builder said so in the first message.", sources = [1] } }, save = "r" },
  { call = { message = { to = "builder", body = "remembered: {{r}}", attachments = [] } } },
]

[[turn]]
when = { input_contains = "STEP-READ" }
repeat = true
steps = [
  { call = { read_note = { note = "deploys" } }, save = "n" },
  { call = { message = { to = "builder", body = "read: {{n}}", attachments = [] } } },
]

[[turn]]
when = { input_contains = "STEP-LOAD" }
repeat = true
steps = [
  { call = { load_skill = { skill = { scope = "shared", name = "deploy" } } }, save = "s" },
  { call = { message = { to = "builder", body = "loaded: {{s}}", attachments = [] } } },
]

[[turn]]
when = { input_contains = "STEP-FINDLOG" }
repeat = true
steps = [
  { call = { search_log = { query = "orange tractor" } }, save = "f" },
  { call = { message = { to = "builder", body = "log: {{f}}", attachments = [] } } },
]

[[turn]]
when = { input_contains = "STEP-FINDNOTES" }
repeat = true
steps = [
  { call = { search_notes = { query = "topic 0003" } }, save = "f" },
  { call = { message = { to = "builder", body = "notes: {{f}}", attachments = [] } } },
]

[[turn]]
when = { input_contains = "STEP-BADSOURCE" }
repeat = true
steps = [
  { call = { remember = { note = "ghost", body = "cites nothing real", sources = [999999] } }, save = "r" },
  { call = { message = { to = "builder", body = "ghost: {{r}}", attachments = [] } } },
]

[[turn]]
repeat = true
steps = [
  { call = { message = { to = "builder", body = "ok", attachments = [] } } },
]
"##;

struct Rig {
    env: Env,
    daemon: DaemonProc,
    client: Client,
    project: ProjectId,
    ceo: AgentId,
    dir: PathBuf,
}

async fn rig(name: &str) -> Rig {
    let env = Env::new();
    std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
    let daemon = env.spawn();
    let client = env.client().await;
    let project = create_project(&client, &env, name).await;
    let ceo = client.project(project).unwrap().project().unwrap().ceo;
    let dir = PathBuf::from(env.project_dir(name));
    Rig {
        env,
        daemon,
        client,
        project,
        ceo,
        dir,
    }
}

impl Rig {
    /// The builder says `body`, and the CEO's answer to it is returned.
    async fn say(&self, body: &str) -> String {
        let before = self.answers();
        self.client
            .send_chat(self.project, body, vec![], None)
            .await
            .unwrap();
        self.client
            .wait_for(WAIT, |s| {
                let answers: Vec<String> = replies(s.projects.get(&self.project)?);
                (answers.len() > before).then(|| answers.last().cloned())
            })
            .await
            .expect("the CEO answers")
            .expect("an answer")
    }

    fn answers(&self) -> usize {
        self.client
            .project(self.project)
            .map_or(0, |p| replies(&p).len())
    }

    /// Stops the CEO's driver once its turn is over, so the next message starts a new one.
    async fn replace_driver(&self) -> DriverId {
        let old = self
            .client
            .wait_for(WAIT, |s| {
                let p = s.projects.get(&self.project)?;
                let a = p.agent(self.ceo)?;
                let d = p.driver(*a.drivers.last()?)?;
                (a.inbox.is_empty()
                    && d.state.is_live()
                    && d.state != x100_core::driver::DriverState::Running)
                    .then_some(d.driver.id)
            })
            .await
            .expect("the CEO's driver is between turns");
        self.client
            .request(StopDriver {
                project: self.project,
                driver: old,
            })
            .await
            .unwrap();
        old
    }

    /// The start pack of the CEO's newest driver once it has started.
    async fn newest_pack(&self, after: usize) -> String {
        let driver = self
            .client
            .wait_for(WAIT, |s| {
                let a = s.projects.get(&self.project)?.agent(self.ceo)?;
                (a.drivers.len() > after).then(|| *a.drivers.last().unwrap())
            })
            .await
            .expect("a new driver starts");
        // The pack is the driver's first transcript entry, a moment after the driver is recorded.
        let until = std::time::Instant::now() + WAIT;
        loop {
            let read = self
                .client
                .request(ReadStartPack {
                    project: self.project,
                    driver,
                })
                .await;
            match read {
                Ok(pack) => return pack.text,
                Err(e) => {
                    assert!(std::time::Instant::now() < until, "no start pack: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }

    fn drivers(&self) -> usize {
        self.client
            .project(self.project)
            .and_then(|p| p.agent(self.ceo).map(|a| a.drivers.len()))
            .unwrap_or(0)
    }

    async fn events(&self) -> Vec<(Seq, ProjectEvent)> {
        let page = self
            .client
            .request(ReadLog {
                log: LogId::Project(self.project),
                page: Page::OldestFirst {
                    after: None,
                    limit: 5000,
                },
            })
            .await
            .unwrap();
        let LogPage::Project { events } = page else {
            panic!("a project's log")
        };
        events.into_iter().map(|e| (e.seq, e.event)).collect()
    }

    fn agent_repo(&self) -> PathBuf {
        self.dir.join(".x100/agents").join(self.ceo.to_string())
    }
}

/// The part of a start pack that starts at `from` and ends where the first of `until` begins.
fn part<'a>(pack: &'a str, from: &str, until: &[&str]) -> &'a str {
    let start = pack
        .find(from)
        .unwrap_or_else(|| panic!("no {from:?} in {pack}"));
    let rest = &pack[start..];
    let end = until
        .iter()
        .filter_map(|u| rest[from.len()..].find(u).map(|i| i + from.len()))
        .min();
    &rest[..end.unwrap_or(rest.len())]
}

/// What `x100-memory` put in a pack: the index.
fn memory_part(pack: &str) -> &str {
    part(pack, "YOUR MEMORY:\n", &["\nSKILL"])
}

/// The list of skills an agent can load.
fn skill_list(pack: &str) -> &str {
    part(pack, "SKILLS YOU CAN LOAD", &["\n\n"])
}

fn replies(p: &x100_core::fold::ProjectState) -> Vec<String> {
    p.messages()
        .filter(|m| m.message.route == Route::CeoToBuilder)
        .map(|m| m.message.body.clone())
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}

fn shared(name: &str) -> SkillRef {
    SkillRef {
        scope: SkillScope::Shared,
        name: SkillName(name.into()),
    }
}

fn deploy_text(description: &str, body: &str) -> String {
    format!("---\nname: deploy\ndescription: {description}\n---\n\n{body}\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn remember_and_load_skill_round_trip_through_the_files_and_each_is_a_commit_and_an_event() {
    let mut r = rig("roundtrip").await;
    // The builder writes a shared skill; it is a commit on main and an event.
    r.say("hello, to start a driver").await;
    let written = r
        .client
        .write_skill(
            r.project,
            shared("deploy"),
            deploy_text("How to deploy this project.", "Run `make ship`."),
        )
        .await
        .unwrap();
    assert_eq!(written.version.0, 1);
    let skills_repo = r.dir.join(".x100/skills");
    assert_eq!(git(&skills_repo, &["rev-parse", "HEAD"]), written.commit.0);
    assert!(
        std::fs::read_to_string(skills_repo.join("deploy/SKILL.md"))
            .unwrap()
            .contains("Run `make ship`.")
    );

    // remember: the agent's note is a file with its sources, a commit, and an event naming it.
    let told = r.say("STEP-REMEMBER please").await;
    let reply: serde_json::Value =
        serde_json::from_str(told.strip_prefix("remembered: ").expect("the CEO's report")).unwrap();
    assert_eq!(reply["note"], "deploys.md", "{told}");
    let file = std::fs::read_to_string(r.agent_repo().join("memory/deploys.md")).unwrap();
    assert!(file.contains("sources: [1]"), "{file}");
    assert!(file.contains("Deploys go through `make ship`, never by hand."));
    let index = std::fs::read_to_string(r.agent_repo().join("memory/INDEX.md")).unwrap();
    assert!(
        index.contains("[deploys.md](deploys.md): Deploys go through `make ship`, never by hand.")
    );
    assert_eq!(
        git(&r.agent_repo(), &["log", "--format=%s"]),
        "remember deploys.md\nx100: start"
    );
    let head = git(&r.agent_repo(), &["rev-parse", "HEAD"]);
    let events = r.events().await;
    let wrote = events.iter().find_map(|(_, e)| match e {
        ProjectEvent::MemoryWritten {
            agent,
            note,
            commit,
        } => Some((*agent, note.clone(), commit.clone())),
        _ => None,
    });
    assert_eq!(
        wrote,
        Some((
            r.ceo,
            NoteName("deploys.md".into()),
            x100_core::git::CommitId(head)
        ))
    );
    assert!(
        told.contains(&wrote.unwrap().2.0),
        "the reply names the commit"
    );

    // read_note returns what was written, with the events it cites, and the read is an event.
    let read = r.say("STEP-READ please").await;
    assert!(
        read.contains("Deploys go through `make ship`, never by hand."),
        "{read}"
    );
    assert!(read.contains("drawn from log events 1"), "{read}");
    let events = r.events().await;
    let reads: Vec<_> = events
        .iter()
        .filter(|(_, e)| matches!(e, ProjectEvent::NoteRead { note, .. } if note.0 == "deploys.md"))
        .collect();
    assert_eq!(reads.len(), 1, "one read, one event");

    // load_skill returns the SKILL.md on main, and the load is an event with its version.
    let loaded = r.say("STEP-LOAD please").await;
    assert!(loaded.contains("Run `make ship`."), "{loaded}");
    assert!(loaded.contains("version: 1"), "{loaded}");
    let events = r.events().await;
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        ProjectEvent::SkillLoaded { agent, skill, version }
            if *agent == r.ceo && skill.name.0 == "deploy" && version.0 == 1
    )));
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        ProjectEvent::SkillCommitted { skill, trial: None, commit, .. }
            if skill.name.0 == "deploy" && commit.0 == written.commit.0
    )));

    // A note cannot cite an event that is not in the log.
    let ghost = r.say("STEP-BADSOURCE").await;
    assert!(ghost.contains("not in the log"), "{ghost}");
    assert!(!r.agent_repo().join("memory/ghost.md").exists());

    // The protocol's lists show the same files.
    let notes = r
        .client
        .list_memories(r.project, r.ceo)
        .await
        .unwrap()
        .notes;
    assert_eq!(notes.len(), 1);
    assert_eq!(
        (notes[0].name.0.as_str(), notes[0].sources.clone()),
        ("deploys.md", vec![Seq(1)])
    );
    assert!(notes[0].body.starts_with("Deploys go through"));
    let skills = r.client.list_skills(r.project, None).await.unwrap().skills;
    let names: Vec<&str> = skills.iter().map(|s| s.skill.name.0.as_str()).collect();
    assert_eq!(names, ["deploy", "executive-summary"]);
    assert_eq!(skills[0].description, "How to deploy this project.");
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_driver_starts_from_the_index_and_loses_what_the_agent_remembered_not() {
    let mut r = rig("replaced").await;
    r.say("STEP-REMEMBER please").await;
    let before = r.drivers();
    r.replace_driver().await;
    r.say("carry on").await;
    let pack = r.newest_pack(before).await;
    // The index is in the next driver's pack, with the note's first line; its second line, which
    // only the note holds, is not.
    let index = memory_part(&pack);
    assert!(
        index
            .contains("- [deploys.md](deploys.md): Deploys go through `make ship`, never by hand."),
        "{index}"
    );
    assert!(
        !index.contains("The builder said so in the first message."),
        "{index}"
    );
    // The built-in skill is carried in full for the CEO, and the pack says how to use memory.
    assert!(pack.contains("SKILL executive-summary (follow it where it says it applies):"));
    assert!(pack.contains("`read_note`"));
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_start_pack_holds_the_index_and_not_the_notes_and_stays_small_with_a_thousand_notes() {
    let mut r = rig("thousand").await;
    r.say("hello").await;
    let small = r.newest_pack(0).await;

    // A thousand notes, in one commit, as an agent that has been at it a long time would have.
    let mem = Memory::new(&r.dir);
    let changes: Vec<NoteChange> = (0..1000)
        .map(|i| NoteChange::Write {
            name: NoteName(format!("topic-{i:04}")),
            body: format!("Topic {i:04} in one line.\n\nDETAIL-{i:04}: the long part that only the note holds."),
            sources: vec![Seq(1)],
        })
        .collect();
    mem.change_notes(
        r.ceo,
        &Target::Main,
        &changes,
        Timestamp::from_millis(1),
        "a thousand notes",
    )
    .unwrap()
    .expect("committed");
    let before = r.drivers();
    r.replace_driver().await;
    r.say("carry on").await;
    let big = r.newest_pack(before).await;

    let index = memory_part(&big);
    assert!(index.starts_with("YOUR MEMORY:\n# Memory index"), "{index}");
    assert!(index.contains("more notes, not listed here"));
    // The notes were written at one time, so they are listed by name, and the last are left out.
    assert!(index.contains("[topic-0000.md]"), "{index}");
    assert!(!index.contains("[topic-0999.md]"), "{index}");
    assert!(!big.contains("DETAIL-"), "no note's text is in the pack");
    // What memory adds has a ceiling, whatever the count: the index and the skills.
    assert!(
        index.len() <= INDEX_LIMIT + "YOUR MEMORY:\n".len(),
        "{} bytes",
        index.len()
    );
    assert!(small.len() > 1000 && big.len() < small.len() + INDEX_LIMIT + SKILLS_LIMIT + 4000);
    // Well under its share of the model's window: 200000 tokens, half of them, four bytes each.
    assert!(big.len() < 200_000 / 2 * 4);

    // A note the index has no room for is found by search, and the search returns no text of
    // the others.
    let found = r.say("STEP-FINDNOTES please").await;
    assert!(
        found.contains("topic-0003.md: Topic 0003 in one line."),
        "{found}"
    );
    assert!(!found.contains("topic-0004"), "{found}");
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_of_the_log_finds_a_past_message_and_not_its_own_query() {
    let mut r = rig("search").await;
    r.say("My lucky orange tractor is parked behind the barn.")
        .await;
    for i in 0..30 {
        r.say(&format!("filler message {i}")).await;
    }
    let found = r.say("STEP-FINDLOG please").await;
    assert!(
        found.contains(
            "from the builder to the CEO: My lucky orange tractor is parked behind the barn."
        ),
        "{found}"
    );
    // One match: the search's own call, which holds the same words, is not a result.
    assert_eq!(found.matches("orange tractor").count(), 1, "{found}");
    assert_eq!(
        found.matches("\\n#").count() + found.matches("log: #").count(),
        1,
        "{found}"
    );
    r.daemon.terminate();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_driver_gets_the_skill_on_main_and_not_the_one_that_is_only_on_a_trial_branch() {
    let mut r = rig("branches").await;
    r.say("hello").await;
    r.client
        .write_skill(
            r.project,
            shared("deploy"),
            deploy_text("Original way to deploy.", "Run `make ship`."),
        )
        .await
        .unwrap();

    // A change on trial: a branch of the shared repository, as the lab will make one.
    let mem = Memory::new(&r.dir);
    let (trial_version, _) = mem
        .write_skill(
            &shared("deploy"),
            &deploy_text("Trial way to deploy.", "Run `make ship --fast`."),
            &Target::Branch("trial/faster".into()),
            "faster deploy",
        )
        .unwrap()
        .unwrap();
    assert_eq!(trial_version.0, 2);

    let before = r.drivers();
    r.replace_driver().await;
    let loaded = r.say("STEP-LOAD while the change is on trial").await;
    let on_trial = r.newest_pack(before).await;
    let listed = skill_list(&on_trial);
    assert!(
        listed.contains("- deploy (version 1, shared): Original way to deploy."),
        "{listed}"
    );
    assert!(
        !on_trial.contains("Trial way to deploy."),
        "the branch is not in the pack"
    );
    assert!(
        loaded.contains("Run `make ship`.") && !loaded.contains("--fast"),
        "{loaded}"
    );

    // The same change reaches main, as a commit and an event, through the builder's call.
    let on_main = r
        .client
        .write_skill(
            r.project,
            shared("deploy"),
            deploy_text("New way to deploy.", "Run `make ship --fast`."),
        )
        .await
        .unwrap();
    assert_eq!(on_main.version.0, 2);
    let before = r.drivers();
    r.replace_driver().await;
    r.say("carry on").await;
    let after = r.newest_pack(before).await;
    assert!(
        skill_list(&after).contains("- deploy (version 2, shared): New way to deploy."),
        "{after}"
    );
    assert!(!skill_list(&after).contains("Original way to deploy."));

    // The branch is still there, and only the builder's two writes to main are events.
    assert_eq!(
        mem.branches(SkillScope::Shared).unwrap(),
        vec!["trial/faster"]
    );
    let events = r.events().await;
    let committed = events
        .iter()
        .filter(|(_, e)| matches!(e, ProjectEvent::SkillCommitted { skill, .. } if skill.name.0 == "deploy"))
        .count();
    assert_eq!(
        committed, 2,
        "each of the builder's writes is an event; the branch is none"
    );
    let commits = git(&r.dir.join(".x100/skills"), &["log", "--format=%s", "main"]);
    assert_eq!(
        commits.lines().collect::<Vec<_>>(),
        [
            "skill deploy: written by the builder",
            "skill deploy: written by the builder",
            "x100: built-in skill",
            "x100: start"
        ]
    );
    r.daemon.terminate();
    assert!(r.env.recorded_pids().is_empty(), "no driver left running");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_builders_skill_writes_are_checked() {
    let mut r = rig("checked").await;
    r.say("hello").await;
    let bad = r
        .client
        .write_skill(r.project, shared("deploy"), "no front matter".into())
        .await;
    assert!(
        matches!(bad, Err(x100_client::ClientError::Rpc(ref e)) if e.code == x100_proto::ErrorCode::Rejected),
        "{bad:?}"
    );
    let text = deploy_text("Deploys.", "Steps.");
    r.client
        .write_skill(r.project, shared("deploy"), text.clone())
        .await
        .unwrap();
    let again = r
        .client
        .write_skill(r.project, shared("deploy"), text)
        .await;
    assert!(
        matches!(again, Err(x100_client::ClientError::Rpc(ref e)) if e.code == x100_proto::ErrorCode::Rejected),
        "{again:?}"
    );
    let nobody = SkillRef {
        scope: SkillScope::Agent(AgentId::from_u128(404)),
        name: SkillName("x".into()),
    };
    let missing = r
        .client
        .write_skill(r.project, nobody, deploy_text("d", "b"))
        .await;
    assert!(
        matches!(missing, Err(x100_client::ClientError::Rpc(ref e)) if e.code == x100_proto::ErrorCode::NotFound),
        "{missing:?}"
    );
    r.daemon.terminate();
}
