//! M10's check with the real `claude` and `codex` programs, each a terminal driver in its agent's
//! sandbox, the model played by the fake provider (see `common/real.rs` for how nothing leaves
//! the machine). The model's shell command tries to delete a stand-in for the builder's home (with
//! `find -delete`, since Codex refuses `rm -rf` by a rule of its own),
//! write outside its directory and connect out directly, and only touches the sandbox; it starts
//! a server with `start_process`. A new driver for the agent finds the file the last one left and
//! the server still serving. Stopping the daemon leaves nothing running.

mod common;

use std::net::TcpListener;

use common::gone;
use common::real::{Program, Real};
use serde_json::json;
use x100_core::agent::AgentState;
use x100_core::id::{AgentId, ProjectId};
use x100_core::process::ProcessName;
use x100_core::transcript::Item;
use x100_drivers::testing::Reply;
use x100_proto::{ReadTranscript, StopDriver};

/// A shell command as each program's model asks for one.
fn shell(program: Program, command: &str) -> Reply {
    match program {
        Program::Claude => Reply::Tool {
            name: "Bash".into(),
            input: json!({ "command": command, "description": "Run a command" }),
        },
        Program::Codex => Reply::Tool {
            name: "exec_command".into(),
            input: json!({ "cmd": command }),
        },
    }
}

/// A call of one of x100's tools as each program's model makes one.
fn x100_tool(program: Program, tool: &str, input: serde_json::Value) -> Reply {
    match program {
        Program::Claude => Reply::Tool {
            name: format!("mcp__x100__{tool}"),
            input,
        },
        // On gpt-6-luna Codex calls MCP tools from its one JavaScript `exec` tool.
        Program::Codex => Reply::Freeform {
            name: "exec".into(),
            input: format!(
                "const r = await tools.mcp__x100__{tool}({input}); text(JSON.stringify(r));"
            ),
        },
    }
}

async fn results(r: &Real, project: ProjectId, agent: AgentId) -> String {
    r.client
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
        .filter_map(|e| match e.entry.item {
            Item::ToolResult { output, .. } => Some(output),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Waits until the CEO is idle on its `n`th driver.
async fn idle_on(r: &Real, ceo: AgentId, n: usize) {
    r.until("the CEO is idle", |p| {
        let a = p.agent(ceo)?;
        (a.state == AgentState::Idle && a.drivers.len() == n).then_some(())
    })
    .await;
}

async fn contained(program: Program) {
    let Some(mut r) = Real::start(program).await else {
        return;
    };
    let home = r.dir.path().join("builder-home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "not a real key").unwrap();
    let outside = r.dir.path().join("outside.txt");
    let host = TcpListener::bind("127.0.0.1:0").unwrap();
    host.set_nonblocking(true).unwrap();
    let port = host.local_addr().unwrap().port();
    let web = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let project = r.project().await;
    r.client
        .send_chat(project, "probe", vec![], None)
        .await
        .unwrap();
    let ceo = r.ceo_waits_for_a_driver().await;
    let probe = format!(
        "find {home} -delete 2>/dev/null; echo escaped > {outside} 2>/dev/null && echo wrote-outside; \
         bash -c 'echo hi > /dev/tcp/127.0.0.1/{port}' 2>/dev/null && echo reached-the-host; \
         curl -s --noproxy '*' -m 3 http://1.1.1.1/ >/dev/null 2>&1 && echo online; \
         echo served-by-the-first > page.txt; echo host-$(hostname)",
        home = home.display(),
        outside = outside.display(),
    );
    let server = format!("exec /usr/bin/python3 -m http.server {web} --bind 127.0.0.1");
    for reply in [
        shell(program, &probe),
        x100_tool(
            program,
            "start_process",
            json!({ "name": "web", "command": server }),
        ),
        Reply::Text("Probed.".into()),
    ] {
        r.provider.reply(reply);
    }
    r.allow(project).await;
    idle_on(&r, ceo, 1).await;
    let said = results(&r, project, ceo).await;
    assert!(said.contains("host-x100"), "{said}");
    for escaped in ["wrote-outside", "reached-the-host", "online"] {
        assert!(!said.contains(escaped), "{said}");
    }
    assert_eq!(
        std::fs::read_to_string(home.join(".ssh/id_ed25519")).unwrap(),
        "not a real key"
    );
    assert!(!outside.exists());
    assert!(
        host.accept().is_err(),
        "a direct connection reached the host"
    );
    let s = r.client.project(project).unwrap();
    assert!(
        s.agent(ceo)
            .unwrap()
            .processes
            .contains_key(&ProcessName("web".into())),
        "{said}"
    );

    // The next driver finds the file and the server the first left.
    let first = *s.agent(ceo).unwrap().drivers.last().unwrap();
    r.client
        .request(StopDriver {
            project,
            driver: first,
        })
        .await
        .unwrap();
    for reply in [
        shell(
            program,
            &format!(
                "cat page.txt; for i in 1 2 3 4 5 6 7 8 9 10; do curl -s -m 1 \
                 http://127.0.0.1:{web}/page.txt && break; sleep 0.5; done | sed 's/^/fetched: /'"
            ),
        ),
        Reply::Text("Fetched.".into()),
    ] {
        r.provider.reply(reply);
    }
    r.client
        .send_chat(project, "again", vec![], None)
        .await
        .unwrap();
    idle_on(&r, ceo, 2).await;
    let said = results(&r, project, ceo).await;
    assert!(
        said.contains("fetched: served-by-the-first"),
        "the next driver reached the server: {said}"
    );

    // Stopping the daemon destroys the sandbox with everything in it.
    let anchors: Vec<u32> = sandboxes(&r);
    assert_eq!(anchors.len(), 1, "one sandbox, the CEO's");
    r.daemon.terminate();
    gone(&anchors).await;
    let marker = format!("http.server {web}");
    let out = std::process::Command::new("ps")
        .args(["-eo", "args="])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains(&marker),
        "the server outlived its sandbox"
    );
    assert_eq!(r.provider.unused(), 0, "every scripted reply was asked for");
}

/// The sandboxes' anchors the daemon wrote down.
fn sandboxes(r: &Real) -> Vec<u32> {
    let text = std::fs::read_to_string(r.data.join("drivers.json")).unwrap();
    let map: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
        serde_json::from_str(&text).unwrap();
    map.iter()
        .filter(|(k, _)| k.starts_with("sandbox "))
        .flat_map(|(_, v)| v.iter().map(|e| e["pid"].as_u64().unwrap() as u32))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_code_driver_is_contained_and_its_server_outlives_it() {
    contained(Program::Claude).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_driver_is_contained_and_its_server_outlives_it() {
    contained(Program::Codex).await;
}
