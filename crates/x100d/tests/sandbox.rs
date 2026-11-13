//! M10's check through the daemon as a child process, on the fake harness: every driver runs in its
//! agent's sandbox. A driver's command that tries to delete a stand-in for the builder's home,
//! write outside its worktree and connect out directly fails or touches only the sandbox, and what
//! it prints reaches the transcript. Two agents' sandboxes cannot see each other's files. Accepting
//! the builder's task merges the CEO's work into the project. A new driver finds the files the
//! last one left and the server it started with `start_process` still serving. Stopping the daemon
//! destroys the sandboxes and leaves nothing running, and the log says the server ended with its
//! sandbox.

mod common;

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use common::{Env, WAIT, create_project, gone, running};
use x100_client::Client;
use x100_core::driver::DriverState;
use x100_core::event::ProjectEvent;
use x100_core::id::{AgentId, ProjectId};
use x100_core::process::{ProcessEnd, ProcessName};
use x100_core::task::TaskState;
use x100_core::transcript::Item;
use x100_proto::{LogId, LogPage, Page, ReadLog, ReadTranscript, StopDriver};

const SCRIPT: &str = r#"
[harness]
name = "fake-anthropic"

[[model]]
model = "fake-small"
context_window = 200000
efforts = ["low", "medium", "high"]
price = { input = 1.0, output = 5.0 }

# What a model might try against the machine, each attempt's outcome printed.
[[turn]]
when = { input_contains = "ESCAPE" }
repeat = true
steps = [
  { run = "rm -rf {{input:Home}} 2>/dev/null; ls {{input:Home}} >/dev/null 2>&1 && echo home-still-there-inside; echo escaped > {{input:Outside}} 2>/dev/null && echo wrote-outside; bash -c 'echo hi > /dev/tcp/127.0.0.1/{{input:Port}}' 2>/dev/null && echo reached-the-host; curl -s --noproxy '*' -m 3 http://1.1.1.1/ >/dev/null 2>&1 && echo online; echo host-$(hostname); touch ran.txt" },
  { call = { message = { to = "builder", body = "probed", attachments = [] } } },
]

[[turn]]
when = { input_contains = "SECRET" }
repeat = true
steps = [
  { write = "secret.txt", content = "only mine" },
  { call = { message = { to = "builder", body = "kept", attachments = [] } } },
]

[[turn]]
when = { input_contains = "PEEK" }
repeat = true
steps = [
  { run = "cat {{input:Peek}} > peeked.txt 2>&1; ls {{input:Dir}} >> peeked.txt 2>&1; true" },
  { call = { message = { to = "builder", body = "peeked", attachments = [] } } },
]

[[turn]]
when = { input_contains = "SERVE" }
repeat = true
steps = [
  { write = "page.txt", content = "served by the last driver" },
  { call = { start_process = { name = "web", command = "exec /usr/bin/python3 -m http.server {{input:Port}} --bind 127.0.0.1" } }, save = "web" },
  { run = "for i in 1 2 3 4 5 6 7 8 9 10; do curl -s -m 1 http://127.0.0.1:{{input:Port}}/page.txt && break; sleep 0.5; done" },
  { call = { message = { to = "builder", body = "serving: {{web}}", attachments = [] } } },
]

[[turn]]
when = { input_contains = "FETCH" }
repeat = true
steps = [
  { run = "cat page.txt > seen.txt; curl -s -m 5 http://127.0.0.1:{{input:Port}}/page.txt > fetched.txt" },
  { call = { message = { to = "builder", body = "fetched", attachments = [] } } },
]

[[turn]]
when = { input_contains = "SHIP" }
repeat = true
steps = [
  { call = { set_up_project = { name = "Shop", intent = "A shop.", brief = "Ship the page." } }, save = "setup" },
  { write = "shipped.txt", content = "shipped" },
  { call = { report = { task = "{{setup.task}}", outcome = "completed", summary = "Shipped." } } },
]

[[turn]]
repeat = true
steps = [{ text = "nothing to do" }]
"#;

fn env() -> Env {
    let env = Env::new();
    std::fs::write(env.scripts.join("fake.toml"), SCRIPT).unwrap();
    env
}

async fn ceo(client: &Client, project: ProjectId) -> AgentId {
    client
        .wait_for(WAIT, |s| s.projects.get(&project)?.project().map(|p| p.ceo))
        .await
        .unwrap()
}

/// Sends `body` and waits for the CEO's message `reply`.
async fn ask(client: &Client, project: ProjectId, body: &str, reply: &str) {
    let before = client
        .project(project)
        .map(|p| p.messages().count())
        .unwrap_or(0);
    client.send_chat(project, body, vec![], None).await.unwrap();
    client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&project)?;
            let said = p
                .messages()
                .skip(before)
                .any(|m| m.message.body.starts_with(reply));
            said.then_some(())
        })
        .await
        .unwrap_or_else(|_| panic!("no {reply:?} after {body:?}"));
}

async fn transcript(client: &Client, project: ProjectId, agent: AgentId) -> Vec<Item> {
    client
        .request(ReadTranscript {
            project,
            agent,
            after: None,
            limit: 1000,
        })
        .await
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.entry.item)
        .collect()
}

fn worktree(env: &Env, name: &str, agent: AgentId) -> PathBuf {
    x100_store::Git::new(PathBuf::from(env.project_dir(name))).worktree_path(agent)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The processes whose command line holds `needle`.
fn processes_with(needle: &str) -> Vec<u32> {
    let out = std::process::Command::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(needle))
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_driver_runs_contained_in_its_agents_sandbox() {
    if common::available::skip_without_sandbox() {
        return;
    }
    let env = env();
    let mut daemon = env.spawn();
    let client = env.client().await;

    // A stand-in for the builder's home, beside the projects, which no sandbox shows.
    let home = env.dir.path().join("builder-home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "not a real key").unwrap();
    let outside = env.dir.path().join("outside.txt");
    let host = TcpListener::bind("127.0.0.1:0").unwrap();
    host.set_nonblocking(true).unwrap();
    let port = host.local_addr().unwrap().port();

    let a = create_project(&client, &env, "a").await;
    let ceo_a = ceo(&client, a).await;
    ask(
        &client,
        a,
        &format!(
            "ESCAPE\nHome: {}\nOutside: {}\nPort: {port}",
            home.display(),
            outside.display()
        ),
        "probed",
    )
    .await;
    let work_a = worktree(&env, "a", ceo_a);
    assert!(work_a.join("ran.txt").exists(), "the command ran");
    assert_eq!(read(&home.join(".ssh/id_ed25519")), "not a real key");
    assert!(!outside.exists(), "a write left the sandbox");
    assert!(
        host.accept().is_err(),
        "a direct connection reached the host"
    );
    // What it printed reached the transcript: only the sandbox's own name.
    let printed: Vec<String> = transcript(&client, a, ceo_a)
        .await
        .into_iter()
        .filter_map(|i| match i {
            Item::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .collect();
    let said = printed.concat();
    assert!(said.contains("host-x100"), "{said}");
    for escaped in [
        "home-still-there-inside",
        "wrote-outside",
        "reached-the-host",
        "online",
    ] {
        assert!(!said.contains(escaped), "{said}");
    }

    // Another agent's sandbox, in another project, cannot see the first one's files.
    ask(&client, a, "SECRET", "kept").await;
    let b = create_project(&client, &env, "b").await;
    let ceo_b = ceo(&client, b).await;
    ask(
        &client,
        b,
        &format!(
            "PEEK\nPeek: {}\nDir: {}",
            work_a.join("secret.txt").display(),
            work_a.display()
        ),
        "peeked",
    )
    .await;
    let peeked = read(&worktree(&env, "b", ceo_b).join("peeked.txt"));
    assert!(!peeked.contains("only mine"), "{peeked}");
    assert_eq!(
        peeked.matches("No such file or directory").count(),
        2,
        "neither the file nor its directory is there: {peeked}"
    );

    // A server started with `start_process` outlives the driver that started it, and the next
    // driver finds it and the files.
    let web_port = {
        let free = TcpListener::bind("127.0.0.1:0").unwrap();
        free.local_addr().unwrap().port()
    };
    ask(&client, a, &format!("SERVE\nPort: {web_port}"), "serving").await;
    let said = client
        .project(a)
        .unwrap()
        .messages()
        .find(|m| m.message.body.starts_with("serving"))
        .map(|m| m.message.body.clone())
        .unwrap();
    assert!(said.contains("\"running\":true"), "{said}");
    let before = client
        .project(a)
        .unwrap()
        .agent(ceo_a)
        .unwrap()
        .drivers
        .clone();
    // The message comes mid-turn. A driver stopped before its turn ends leaves the turn's items
    // for the next one, which would then serve again before it fetched.
    let server = *before.last().unwrap();
    client
        .wait_for(WAIT, |s| {
            let state = &s.projects.get(&a)?.driver(server)?.state;
            (*state == DriverState::Warm).then_some(())
        })
        .await
        .expect("the turn that started the server ends");
    client
        .request(StopDriver {
            project: a,
            driver: server,
        })
        .await
        .unwrap();
    ask(&client, a, &format!("FETCH\nPort: {web_port}"), "fetched").await;
    let s = client.project(a).unwrap();
    let drivers = &s.agent(ceo_a).unwrap().drivers;
    assert_eq!(drivers.len(), before.len() + 1, "a new driver fetched");
    assert_eq!(read(&work_a.join("seen.txt")), "served by the last driver");
    assert_eq!(
        read(&work_a.join("fetched.txt")),
        "served by the last driver"
    );
    assert!(
        s.agent(ceo_a)
            .unwrap()
            .processes
            .contains_key(&ProcessName("web".into()))
    );
    let marker = format!("http.server {web_port}");
    let server = processes_with(&marker);
    assert!(!server.is_empty(), "the server runs");

    // The builder's task, accepted as the CEO reports it, merges its work into the project.
    client.send_chat(a, "SHIP", vec![], None).await.unwrap();
    client
        .wait_for(WAIT, |s| {
            let p = s.projects.get(&a)?;
            p.tasks()
                .any(|t| matches!(t.state, TaskState::Done(..)))
                .then_some(())
        })
        .await
        .expect("the task is done");
    let main = PathBuf::from(env.project_dir("a"));
    assert_eq!(read(&main.join("shipped.txt")), "shipped");

    // Stopping the daemon destroys every sandbox, with what runs in it.
    let anchors: Vec<u32> = env.recorded_sandboxes().iter().map(|(_, p)| *p).collect();
    assert_eq!(anchors.len(), 2, "one sandbox for each CEO");
    daemon.terminate();
    gone(&anchors).await;
    gone(&server).await;
    assert!(processes_with(&marker).iter().all(|p| !running(*p)));
    assert!(
        std::fs::read_dir(env.data.join("sb"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "the sandboxes' own directories are removed"
    );

    // The log says the server ended with its sandbox, and a daemon started again agrees.
    let _daemon = env.spawn();
    let client = env.client().await;
    let page = client
        .request(ReadLog {
            log: LogId::Project(a),
            page: Page::NewestFirst {
                before: None,
                limit: 200,
            },
        })
        .await
        .unwrap();
    let LogPage::Project { events } = page else {
        panic!("a project's log")
    };
    let ended = events.iter().any(|e| {
        matches!(&e.event, ProjectEvent::ProcessStopped { name, end: ProcessEnd::SandboxDestroyed, .. }
            if name.0 == "web")
    });
    assert!(ended, "the server's end is in the log");
    let s = client
        .wait_for(WAIT, |s| s.projects.get(&a).cloned())
        .await
        .unwrap();
    assert!(s.agent(ceo_a).unwrap().processes.is_empty());
}
