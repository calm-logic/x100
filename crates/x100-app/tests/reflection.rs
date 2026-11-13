//! An agent's page shows its memory notes and its skills as the daemon lists them from the
//! repositories, and the trials about it as the daemon lists them, against a scripted daemon on a
//! socket. The state is core's Fernleaf, whose agents have loaded the deploy skill and whose CEO
//! put a change to it on trial; the daemon here lists notes the state knows nothing of, as files a
//! driver wrote after the snapshot would be.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{TestAppContext, prelude::*};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use x100_app::reflection::{Listed, Reflection};
use x100_app::session::Session;
use x100_client::{Client, Options};
use x100_core::event::Seq;
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::git::CommitId;
use x100_core::id::AgentId;
use x100_core::memory::{NoteName, SkillName, SkillRef, SkillScope, SkillVersion};
use x100_core::testing::{FERNLEAF, START, daemon_log, deploy_skill, fernleaf};
use x100_core::time::Timestamp;
use x100_proto::{
    Call, ErrorCode, Id, Memories, MemoryNote, Message, Response, RpcError, SkillInfo, Skills,
    Snapshot, Trials,
};

const WAIT: Duration = Duration::from_secs(20);

struct World {
    snapshot: Snapshot,
    /// An agent that has loaded the deploy skill, and the CEO, who has not.
    agent: AgentId,
    other: AgentId,
}

fn world() -> World {
    let f = fernleaf();
    let project = ProjectState::fold(&f.log).expect("the log folds");
    let agent = project
        .agents()
        .find(|a| !a.skills.is_empty())
        .map(|a| a.id)
        .expect("an agent that has loaded a skill");
    let daemon = DaemonState::fold(&daemon_log(START)).expect("the daemon's log folds");
    World {
        snapshot: Snapshot {
            daemon,
            projects: BTreeMap::from([(FERNLEAF, project)]),
        },
        agent,
        other: f.ceo,
    }
}

fn ok(id: Id, result: serde_json::Value) -> String {
    Message::Response(Response {
        id: Some(id),
        outcome: Ok(result),
    })
    .to_line()
}

fn err(id: Id, code: ErrorCode, message: &str) -> String {
    Message::Response(Response::err(Some(id), RpcError::new(code, message))).to_line()
}

fn skill(scope: SkillScope, name: &str, version: u32, description: &str) -> SkillInfo {
    SkillInfo {
        skill: SkillRef {
            scope,
            name: SkillName(name.into()),
        },
        version: SkillVersion(version),
        description: description.into(),
        body: format!(
            "---\nname: {name}\ndescription: {description}\nversion: {version}\n---\n\nSteps for {name}.\n"
        ),
    }
}

/// What the daemon lists: two notes, one of which the state does not know, and skills of the
/// project and of two agents.
fn memories(w: &World) -> Memories {
    let note = |name: &str, body: &str, sources: Vec<Seq>| MemoryNote {
        name: NoteName(name.into()),
        commit: CommitId("c0ffee".into()),
        written: Timestamp::from_millis(START.as_millis()),
        sources,
        body: body.into(),
    };
    let _ = w;
    Memories {
        notes: vec![
            note("stack.md", "We build on SQLite.", vec![Seq(2), Seq(5)]),
            note("auth.md", "Sessions live in KV.", vec![]),
        ],
    }
}

fn skills(w: &World) -> Skills {
    Skills {
        skills: vec![
            skill(SkillScope::Shared, "deploy", 3, "How to deploy."),
            skill(SkillScope::Shared, "lint", 1, "How to lint."),
            skill(SkillScope::Agent(w.agent), "tuning", 2, "Its own tuning."),
            skill(SkillScope::Agent(w.other), "secret", 1, "Another agent's."),
        ],
    }
}

async fn serve(listener: UnixListener, w: Arc<World>, asked: Arc<Mutex<Vec<String>>>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let (read, mut write) = stream.into_split();
        let (out, mut lines_out) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = lines_out.recv().await {
                write.write_all(line.as_bytes()).await.unwrap();
                write.write_all(b"\n").await.unwrap();
            }
        });
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(Message::Request(request)) = Message::parse(&line) else {
                panic!("not a request: {line}");
            };
            asked.lock().unwrap().push(request.call.method().to_owned());
            let id = request.id.clone();
            let send = |line: String| out.send(line).unwrap();
            match request.call {
                Call::Hello(_) => {
                    send(ok(id, serde_json::json!({"daemon": "fake", "protocol": 1})))
                }
                Call::Subscribe(_) => send(ok(
                    id,
                    serde_json::json!({"subscription": 1, "snapshot": w.snapshot}),
                )),
                Call::ListMemories(m) if m.agent == w.agent => {
                    send(ok(id, serde_json::to_value(memories(&w)).unwrap()))
                }
                Call::ListMemories(_) => send(ok(
                    id,
                    serde_json::to_value(Memories { notes: vec![] }).unwrap(),
                )),
                Call::ListSkills(_) => send(ok(id, serde_json::to_value(skills(&w)).unwrap())),
                Call::ListTrials(_) => {
                    let trials = w.snapshot.projects[&FERNLEAF].trials().cloned().collect();
                    send(ok(id, serde_json::to_value(Trials { trials }).unwrap()))
                }
                Call::Unsubscribe(_) => send(ok(id, serde_json::json!({}))),
                _ => send(err(id, ErrorCode::NotFound, "not scripted")),
            }
        }
    }
}

fn names(cards: &[x100_app::reflection::Card]) -> Vec<&str> {
    cards.iter().map(|c| c.name.as_str()).collect()
}

#[test]
fn an_agents_page_lists_its_notes_and_skills_from_the_repositories() {
    // As the window tests: a plain test, since the client wakes GPUI's tasks from Tokio's threads.
    let mut cx = TestAppContext::single();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let w = Arc::new(world());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("x100d.sock");
    let client = rt.block_on(async {
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(serve(listener, Arc::clone(&w), Arc::clone(&asked)));
        Client::connect(Options::new(&socket, "test"))
            .await
            .unwrap()
    });
    let agent = w.agent;
    let (view, vcx) = cx.add_window_view(move |_, cx| {
        let session = cx.new(|cx| Session::new(client, cx));
        Reflection::new(session, FERNLEAF, agent, cx)
    });
    let vcx = vcx.clone();

    let until = Instant::now() + WAIT;
    let listed: Listed = loop {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        let listed = vcx.read(|cx| view.read(cx).listed(cx));
        let asked_trials = asked.lock().unwrap().contains(&"trials.list".to_owned());
        if listed.notes.iter().all(|n| n.body.is_some()) && listed.skills.len() == 3 && asked_trials
        {
            break listed;
        }
        assert!(
            Instant::now() < until,
            "the page never listed them: {listed:#?}"
        );
    };

    // The notes are the files', including one the state has no record of, with what they cite.
    assert_eq!(names(&listed.notes), ["stack.md", "auth.md"]);
    assert_eq!(
        listed.notes[0].subtitle,
        "memory note \u{b7} cites 2 events"
    );
    assert_eq!(listed.notes[0].body.as_deref(), Some("We build on SQLite."));
    assert_eq!(listed.notes[1].subtitle, "memory note");

    // The skills are the project's and its own, not another agent's, with versions, and the one
    // it has loaded says so.
    assert_eq!(names(&listed.skills), ["deploy", "lint", "tuning"]);
    assert_eq!(
        listed.skills[0].subtitle,
        "shared \u{b7} version 3 \u{b7} loaded"
    );
    assert_eq!(listed.skills[1].subtitle, "shared \u{b7} version 1");
    assert_eq!(listed.skills[2].subtitle, "its own \u{b7} version 2");
    let body = listed.skills[0].body.as_deref().unwrap();
    assert!(
        body.contains("How to deploy.") && body.contains("Steps for deploy."),
        "{body}"
    );
    assert!(
        !body.contains("name: deploy"),
        "the front matter is not shown: {body}"
    );
    assert_eq!(deploy_skill().name.0, "deploy");

    let asked = asked.lock().unwrap().clone();
    assert!(asked.contains(&"memories.list".to_owned()), "{asked:?}");
    assert!(asked.contains(&"skills.list".to_owned()), "{asked:?}");

    // The trial of the deploy skill is about an agent that loaded it, with each arm's scores. The
    // other trial, of another agent's own skill, is not about this one.
    assert_eq!(listed.trials.len(), 1, "{:#?}", listed.trials);
    let t = &listed.trials[0];
    assert_eq!(t.change, "A change to the skill deploy");
    assert_eq!(t.state, "Committed");
    assert!(
        t.without
            .starts_with("Without the change: 6 of 9 checks passed over 3 runs"),
        "{t:#?}"
    );
    assert!(
        t.with
            .starts_with("With the change: 9 of 9 checks passed over 3 runs"),
        "{t:#?}"
    );
    assert_eq!(t.why, None);
}
