//! The daemon's tools over MCP, as a harness's client reaches them: each driver at its own address
//! with its own token, offered the tools it may call, and fenced off once it no longer acts for
//! its agent.

mod common;

use common::mcp::{Answer, Client, PROTOCOL};
use common::*;
use serde_json::{Value, json};
use x100_core::action::Request;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId, TaskId};
use x100_core::spec::{SpecKind, SpecStatus};
use x100_core::tool::ToolCall;
use x100_drivers::ToolEndpoint;
use x100_drivers::fake::DaemonTools;

const SCRIPT: &str = r#"
[harness]
name = "fake-mcp"

[[model]]
model = "fake-long"
context_window = 200000
cache_lifetime_secs = 604800
efforts = ["low", "medium", "high", "max"]
price = { input = 1.0, output = 5.0 }

# The CEO forms a team of one lead, gives it a task and waits on the report.
[[turn]]
when = { start_pack_contains = "ROLE: CEO\n", input_contains = "Two at once" }
steps = [
  { call = { form_team = { leads = [{ role = "Tech", charter = "Builds it.", effort = { min = "low", max = "max" }, budget = 10000000 }] } }, save = "team" },
  { call = { delegate = { to = { child = "{{team.Tech}}" }, task = { new = { title = "Build it", ask = "Build it.", attachments = [] } }, effort = "low" } }, save = "job" },
  { call = { wait = { on = { report = "{{job.task}}" }, task = "{{input:Current task}}", progress = { note = "Waiting on Tech.", done = 10 } } } },
]

# Anyone else waits on a job of its own, far off, so its driver stays warm.
[[turn]]
repeat = true
steps = [
  { call = { schedule = { when = { once = { at = 1794423600000 } }, does = { wake = { agent = "{{pack:Your id}}", note = "Later." } } } }, save = "later" },
  { call = { wait = { on = { job = "{{later.job}}" }, task = "{{input:Current task}}", progress = { note = "Waiting.", done = 0 } } } },
]
"#;

async fn world(brief: &str) -> (World, TaskId) {
    let mut setup = Setup::new(SCRIPT);
    setup.serve = true;
    let w = World::new(setup).await;
    let task = w.send(brief).await;
    w.settle().await;
    (w, task)
}

fn acting(s: &ProjectState, agent: AgentId) -> DriverId {
    s.acting_driver(agent).expect("the agent has a driver")
}

fn endpoint(w: &World, driver: DriverId) -> ToolEndpoint {
    w.endpoints.lock().unwrap()[&driver].clone()
}

/// The tools a later milestone builds, which do nothing yet.
const LATER: [&str; 7] = [
    "start_process",
    "stop_process",
    "load_skill",
    "remember",
    "read_memories",
    "read_log",
    "propose_change",
];
const CEO_ONLY: [&str; 4] = ["form_team", "set_effort", "set_budget", "set_up_project"];

fn parsed(answer: &Answer) -> Value {
    assert!(!answer.is_error, "{answer:?}");
    serde_json::from_str(&answer.text).expect("the reply is JSON")
}

/// The driver each of the agent's tool calls was recorded as coming from, in order, with the tool.
fn calls_by(s: &ProjectState, agent: AgentId) -> Vec<(DriverId, String)> {
    s.actions()
        .filter(|a| a.action.agent == agent)
        .filter_map(|a| match &a.action.request {
            Request::Tool { driver, call } => Some((*driver, kind_of(call))),
            _ => None,
        })
        .collect()
}

fn kind_of(call: &ToolCall) -> String {
    match serde_json::to_value(call).unwrap() {
        Value::Object(m) => m.keys().next().unwrap().clone(),
        other => panic!("{other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_drivers_reach_their_own_tools_over_http_at_once() {
    let (w, _) = world("Two at once").await;
    let s = w.state();
    let tech = agent(&s, "Tech");
    let tech_task = s
        .tasks()
        .find(|t| t.state.assignment().is_some_and(|a| a.assignee == tech))
        .expect("Tech's task")
        .task
        .id;
    let (ceo_driver, tech_driver) = (acting(&s, w.ceo), acting(&s, tech));
    let ceo = Client::new(&endpoint(&w, ceo_driver));
    let lead = Client::new(&endpoint(&w, tech_driver));
    let base = w.tools.as_ref().unwrap().url();
    assert!(base.starts_with("http://127.0.0.1:"), "{base}");
    assert_eq!(
        endpoint(&w, ceo_driver).url,
        format!("{base}/drivers/{ceo_driver}/mcp")
    );
    assert_ne!(endpoint(&w, ceo_driver), endpoint(&w, tech_driver));

    let ceo_side = async {
        let info = ceo.initialize().await;
        let tools = ceo.tools().await.expect("the CEO's tools");
        let spec = ceo
            .call(
                "write_spec",
                json!({ "spec": "brd", "status": "draft", "body": "# BRD\nFrom the CEO." }),
            )
            .await;
        let wait = ceo
            .call(
                "wait",
                json!({
                    "on": { "report": tech_task },
                    "task": s.current_task(w.ceo).unwrap().task.id,
                    "progress": { "note": "Still waiting on Tech.", "done": 20 },
                }),
            )
            .await;
        (info, tools, spec, wait)
    };
    let lead_side = async {
        let info = lead.initialize().await;
        let names = lead.names().await;
        let update = lead
            .call(
                "update_task",
                json!({ "task": tech_task, "progress": { "note": "Half built.", "done": 50 } }),
            )
            .await;
        let form = lead
            .call(
                "form_team",
                json!({ "leads": [{ "role": "Ops", "charter": "Runs it.", "effort": { "min": "low", "max": "max" }, "budget": 1 }] }),
            )
            .await;
        (info, names, update, form)
    };
    let ((info, tools, spec, wait), (lead_info, lead_names, update, form)) =
        tokio::join!(ceo_side, lead_side);

    for i in [&info, &lead_info] {
        assert_eq!(i["protocolVersion"], PROTOCOL);
        assert_eq!(i["serverInfo"]["name"], "x100");
        assert!(i["capabilities"]["tools"].is_object(), "{i}");
    }
    let ceo_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for t in &tools {
        assert!(
            t["description"].as_str().is_some_and(|d| d.len() > 40),
            "{t}"
        );
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
    }
    for name in CEO_ONLY {
        assert!(ceo_names.contains(&name), "the CEO is offered {name}");
        assert!(
            !lead_names.iter().any(|n| n == name),
            "a lead is not offered {name}"
        );
    }
    for name in LATER {
        assert!(!ceo_names.contains(&name), "{name} is not offered yet");
        assert!(
            !lead_names.iter().any(|n| n == name),
            "{name} is not offered yet"
        );
    }
    let mut expected: Vec<&str> = ceo_names
        .iter()
        .copied()
        .filter(|n| !CEO_ONLY.contains(n))
        .collect();
    let mut got: Vec<&str> = lead_names.iter().map(String::as_str).collect();
    expected.sort();
    got.sort();
    assert_eq!(got, expected, "a lead gets every other tool the CEO gets");
    assert_eq!(ceo_names.len(), 16);

    assert_eq!(parsed(&spec)["revision"], 1);
    assert_eq!(parsed(&update)["task"], json!(tech_task));
    let waited = parsed(&wait);
    assert!(waited["wait"].is_string(), "{waited}");
    assert!(
        waited["next"].as_str().unwrap().contains("end the turn"),
        "a wait tells the model its turn is over: {waited}"
    );
    assert!(form.is_error);
    assert!(form.text.contains("only the CEO"), "{form:?}");

    let s = w.state();
    assert_eq!(
        s.spec(SpecKind::Brd).unwrap().spec.body,
        "# BRD\nFrom the CEO."
    );
    assert_eq!(
        s.task(tech_task).unwrap().progress.as_ref().unwrap().note,
        "Half built."
    );
    let ceo_calls = calls_by(&s, w.ceo);
    assert!(ceo_calls.contains(&(ceo_driver, "write_spec".into())));
    assert!(calls_by(&s, tech).contains(&(tech_driver, "update_task".into())));

    // What a harness gets wrong is answered as an error the model can read, and changes nothing.
    let seq = s.seq();
    let unknown = ceo.call("deploy", json!({})).await;
    assert!(unknown.is_error && unknown.text.contains("no tool `deploy`"));
    let wrong = ceo
        .call("write_spec", json!({ "spec": "novel", "body": 3 }))
        .await;
    assert!(
        wrong.is_error && wrong.text.contains("do not fit `write_spec`"),
        "{wrong:?}"
    );
    assert_eq!(w.state().seq(), seq);
    w.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_driver_is_fenced_off_and_its_successor_acts() {
    let (w, _) = world("Get started.").await;
    let first = acting(&w.state(), w.ceo);
    let old = endpoint(&w, first);
    w.orch.stop_driver(w.project, first).await.expect("stopped");
    w.orch
        .send_chat(w.project, "Carry on.".into(), vec![], None)
        .await
        .expect("sent");
    w.settle().await;
    let second = acting(&w.state(), w.ceo);
    assert_ne!(first, second);
    let new = endpoint(&w, second);

    let brd = json!({ "spec": "brd", "status": "draft", "body": "# BRD" });
    let seq = w.state().seq();
    let stale = Client::new(&old);
    let refused = stale.call("write_spec", brd.clone()).await;
    assert!(refused.is_error, "{refused:?}");
    assert!(refused.text.contains("has ended"), "{refused:?}");
    assert!(
        stale.tools().await.is_err(),
        "a fenced driver is offered nothing"
    );
    // The fake harness's path is the same way in, and is fenced the same way.
    let call = ToolCall::WriteSpec {
        spec: SpecKind::Brd,
        status: SpecStatus::Draft,
        body: "# BRD".into(),
    };
    let in_process = DaemonTools::call(&*w.desk, first, Some(old.clone()), call).await;
    assert!(in_process.is_err_and(|e| e.contains("has ended")));

    let made_up = Client::at(&new.url, "not-a-token");
    let forged = made_up.call("write_spec", brd.clone()).await;
    assert!(
        forged.is_error && forged.text.contains("not given to"),
        "{forged:?}"
    );
    let nobody = Client::at(
        &new.url
            .replace(&second.to_string(), &DriverId::from_u128(99).to_string()),
        &new.token,
    );
    assert!(nobody.call("write_spec", brd.clone()).await.is_error);

    assert_eq!(w.state().seq(), seq, "refused calls change nothing");
    assert!(w.state().spec(SpecKind::Brd).is_none());
    let refusals = w.desk.refusals();
    for (driver, why) in [
        (first, "has ended"),
        (second, "not given to"),
        (DriverId::from_u128(99), "not given to"),
    ] {
        assert!(
            refusals.iter().any(|r| r.driver == driver
                && r.tool.as_deref() == Some("write_spec")
                && r.why.contains(why)),
            "{driver} refused for {why:?}: {refusals:#?}"
        );
    }

    let accepted = Client::new(&new).call("write_spec", brd).await;
    assert_eq!(parsed(&accepted)["revision"], 1);
    let s = w.state();
    assert_eq!(s.spec(SpecKind::Brd).unwrap().spec.body, "# BRD");
    assert!(calls_by(&s, w.ceo).contains(&(second, "write_spec".into())));
    assert!(
        !calls_by(&s, w.ceo)
            .iter()
            .any(|(d, t)| *d == first && t == "write_spec")
    );
    w.finish().await;
}
