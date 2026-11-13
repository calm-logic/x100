//! The single commands, run as the real binary against a real daemon on the fake clock, alone and
//! in shell pipelines: what they print is what the daemon holds, and their exit codes say how they
//! ended.

mod common;

use std::process::{Command, Output, Stdio};

use common::*;
use x100_core::action::{ActionState, AskMeAbout};
use x100_core::harness::{HarnessKind, HarnessModel, ModelName};
use x100_core::project::SettingChange;
use x100_proto::ChangeProjectSetting;

const X100: &str = env!("CARGO_BIN_EXE_x100");

fn x100(run: &Run, args: &[&str]) -> Output {
    Command::new(X100)
        .arg("--socket")
        .arg(run.daemon.socket())
        .args(args)
        .env_remove("X100_PROJECT")
        .stdin(Stdio::null())
        .output()
        .expect("x100 runs")
}

/// A shell pipeline with `x100` standing for the binary on this run's daemon.
fn pipeline(run: &Run, script: &str) -> Output {
    Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -o pipefail; x100() {{ \"$X100\" --socket \"$SOCK\" \"$@\"; }}; {script}"
        ))
        .env("X100", X100)
        .env("SOCK", run.daemon.socket())
        .env("BRIEF", BRIEF)
        .env_remove("X100_PROJECT")
        .stdin(Stdio::null())
        .output()
        .expect("bash runs")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exited")
}

#[test]
fn a_pipeline_makes_a_project_talks_to_its_ceo_and_reads_the_log() {
    let run = Run::start();
    run.long_waits();
    // The project is made by a command, and the script's long-lived model is turned off by the
    // test so the run is the one the daemon's own Fernleaf test settles.
    let made = x100(&run, &["new"]);
    assert_eq!(code(&made), 0, "{}", err(&made));
    let id = out(&made).trim().to_owned();
    let project: x100_core::id::ProjectId = id.parse().expect("a project id on stdout");
    let long = run.client.with_state(|s| {
        let harness = s
            .daemon
            .harnesses()
            .find(|h| {
                h.kind
                    == HarnessKind::Custom {
                        name: "fake-fernleaf".into(),
                    }
            })
            .map(|h| h.id)
            .unwrap();
        HarnessModel {
            harness,
            model: ModelName("fake-long".into()),
        }
    });
    run.rt
        .block_on(
            run.client
                .wait_for(WAIT, |s| s.projects.get(&project).map(|_| ())),
        )
        .unwrap();
    run.rt
        .block_on(run.client.change_setting(
            project,
            SettingChange::Allow {
                what: long,
                allowed: false,
            },
        ))
        .unwrap();

    // The brief comes from stdin, and the chat is followed until the CEO asks its question.
    let o = pipeline(
        &run,
        &format!(
            "printf '%s' \"$BRIEF\" | x100 send --project {id} - >/dev/null \
             && x100 chat --project {id} -f --until 'Forest greens' --timeout 60"
        ),
    );
    assert_eq!(code(&o), 0, "{}", err(&o));
    let chat = out(&o);
    let rows: Vec<Vec<&str>> = chat.lines().map(|l| l.split('\t').collect()).collect();
    assert!(rows.iter().all(|r| r.len() == 3), "{chat}");
    assert_eq!(rows[0][1], "you");
    assert!(
        rows[0][2].starts_with("Launch Fernleaf\\nA landing page"),
        "{chat}"
    );
    let last = rows.last().unwrap();
    assert_eq!((last[1], last[2]), ("ceo", "Forest greens or ocean blues?"));

    // The daemon holds what the commands said.
    run.settle(project);
    let p = run.client.project(project).expect("the project");
    assert_eq!(p.project().unwrap().name(), Some("Fernleaf"));

    // The log: every event once, oldest first, four fields, the last event last.
    let all = x100(&run, &["log", "--project", &id, "--all"]);
    assert_eq!(code(&all), 0, "{}", err(&all));
    let log = out(&all);
    let seqs: Vec<u64> = log
        .lines()
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 4, "{l}");
            f[0].parse().unwrap()
        })
        .collect();
    let settled = p.seq().unwrap().0;
    let want = *seqs.last().unwrap();
    assert!(want >= settled, "{want} < {settled}");
    assert_eq!(seqs, (1..=want).collect::<Vec<_>>());
    // `-n` takes the newest few, still oldest first.
    let few = out(&x100(&run, &["log", "--project", &id, "-n", "3"]));
    let few: Vec<u64> = few
        .lines()
        .map(|l| l.split('\t').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(few.len(), 3);
    assert!(
        few[0] + 1 == few[1] && few[1] + 1 == few[2] && few[2] >= want,
        "{few:?}"
    );

    // A reader that quits early is not an error: `head` closes the pipe.
    let early = pipeline(&run, &format!("x100 log --project {id} --all | head -2"));
    assert_eq!(code(&early), 0, "{}", err(&early));
    assert_eq!(out(&early).lines().count(), 2);
    // Following ends when the text is seen, and times out with its own code when it is not.
    let seen = x100(
        &run,
        &[
            "log",
            "--project",
            &id,
            "-f",
            "--until",
            "Charged",
            "--timeout",
            "30",
        ],
    );
    assert_eq!(code(&seen), 0, "{}", err(&seen));
    let never = x100(
        &run,
        &[
            "log",
            "--project",
            &id,
            "-f",
            "--until",
            "no such line",
            "--timeout",
            "1",
        ],
    );
    assert_eq!(code(&never), 4);

    // The project list.
    let list = out(&x100(&run, &["projects"]));
    let f: Vec<&str> = list.trim_end().split('\t').collect();
    assert_eq!(f[0], id);
    assert_eq!(f[1], "Fernleaf");
    assert_eq!(f[2], p.agent_count().to_string());
    assert!(f[3] == "live" || f[3] == "idle");
    // With one project, none needs naming.
    let chat_again = x100(&run, &["chat"]);
    assert_eq!(code(&chat_again), 0);
    assert_eq!(
        out(&chat_again).lines().count(),
        run.client.project(project).unwrap().chat().len()
    );

    // Sending answers the CEO's question, the message id is printed, and the daemon holds it.
    let sent = pipeline(
        &run,
        &format!("printf 'Forest greens' | x100 send --project {id} -"),
    );
    assert_eq!(code(&sent), 0, "{}", err(&sent));
    let message = out(&sent).trim().to_owned();
    let m = run
        .rt
        .block_on(run.client.wait_for(WAIT, |s| {
            s.projects
                .get(&project)?
                .messages()
                .find(|m| m.message.id.to_string() == message)
                .cloned()
        }))
        .expect("the daemon holds it");
    assert_eq!(m.message.body, "Forest greens");
    assert!(m.message.in_reply_to.is_some(), "it answered the question");

    // Following prints what happens after it started: tasks raised while it runs are its lines.
    let mut follower = Command::new(X100)
        .arg("--socket")
        .arg(run.daemon.socket())
        .args([
            "log",
            "--project",
            &id,
            "-n",
            "0",
            "-f",
            "--until",
            "Follow me",
            "--timeout",
            "30",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("x100 runs");
    let until = std::time::Instant::now() + WAIT;
    let status = loop {
        run.rt
            .block_on(
                run.client
                    .raise_task(project, "Follow me", "From the test", None),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        if let Some(status) = follower.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < until,
            "the follower never ended"
        );
    };
    let mut printed = String::new();
    std::io::Read::read_to_string(&mut follower.stdout.take().unwrap(), &mut printed).unwrap();
    assert!(status.success(), "{status:?}");
    let last = printed.lines().last().expect("a line");
    assert!(
        last.contains("Follow me") && last.split('\t').count() == 4,
        "{printed}"
    );
}

#[test]
fn exit_codes_say_how_a_command_ended() {
    let run = Run::start();
    run.long_waits();
    let usage = x100(&run, &["frobnicate"]);
    assert_eq!(code(&usage), 2);
    assert!(err(&usage).contains("no command frobnicate"));
    assert_eq!(code(&x100(&run, &["send"])), 2);
    // No project yet: a refusal, not a usage error.
    let none = x100(&run, &["send", "hello"]);
    assert_eq!(code(&none), 1);
    assert!(err(&none).contains("no project"), "{}", err(&none));
    // Nothing answers on a socket nobody listens on.
    let gone = Command::new(X100)
        .args([
            "--socket",
            "/nonexistent/x100-test/x100d.sock",
            "--connect-timeout",
            "1",
            "projects",
        ])
        .output()
        .unwrap();
    assert_eq!(code(&gone), 3, "{}", err(&gone));
    // Two projects: the command must be told which.
    let a = out(&x100(&run, &["new"]));
    let b = out(&x100(&run, &["new"]));
    assert_ne!(a, b);
    let which = x100(&run, &["chat"]);
    assert_eq!(code(&which), 2);
    assert!(err(&which).contains("--project"));
    // An id for something that is not there.
    let missing = x100(
        &run,
        &[
            "approve",
            "act_01M74XBRM0TJ9RTX4RWHN7NK5C",
            "--project",
            a.trim(),
        ],
    );
    assert_eq!(code(&missing), 1, "{}", err(&missing));
    assert_eq!(
        out(&x100(&run, &["help"])).lines().next().unwrap(),
        "usage: x100 [--socket PATH] [--daemon PATH] [--connect-timeout SECS] [COMMAND]"
    );
}

#[test]
fn held_actions_are_listed_and_decided_from_the_command_line() {
    let run = Run::start();
    let project = run.fernleaf_project();
    run.rt
        .block_on(run.client.request(ChangeProjectSetting {
            project,
            change: SettingChange::AskMeAbout(AskMeAbout::EveryDecision),
        }))
        .unwrap();
    let id = project.to_string();
    let sent = x100(&run, &["send", "--project", &id, BRIEF]);
    assert_eq!(code(&sent), 0, "{}", err(&sent));
    run.rt
        .block_on(run.client.wait_for(WAIT, |s| {
            (s.projects.get(&project)?.held_actions().count() == 2).then_some(())
        }))
        .expect("two actions wait");

    let listed = out(&x100(&run, &["actions", "--project", &id]));
    let rows: Vec<Vec<&str>> = listed.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows.len(), 2, "{listed}");
    assert!(
        rows.iter().all(|r| r.len() == 4 && r[1] == "CEO"),
        "{listed}"
    );
    let p = run.client.project(project).unwrap();
    let held: Vec<String> = p.held_actions().map(|a| a.action.id.to_string()).collect();
    assert!(rows.iter().all(|r| held.contains(&r[0].to_owned())));

    let approved = x100(&run, &["approve", rows[0][0], "--project", &id]);
    assert_eq!(code(&approved), 0, "{}", err(&approved));
    assert_eq!(out(&approved).trim(), format!("{}\tapproved", rows[0][0]));
    let denied = x100(&run, &["deny", rows[1][0], "--project", &id]);
    assert_eq!(code(&denied), 0, "{}", err(&denied));
    let p = run.client.project(project).unwrap();
    let state = |s: &str| {
        p.actions()
            .find(|a| a.action.id.to_string() == s)
            .unwrap()
            .state
    };
    assert_eq!(state(rows[1][0]), ActionState::Denied);
    assert!(matches!(
        state(rows[0][0]),
        ActionState::Approved | ActionState::CarriedOut
    ));
    // Deciding twice is a refusal.
    assert_eq!(
        code(&x100(&run, &["deny", rows[1][0], "--project", &id])),
        1
    );
}

#[test]
fn new_takes_its_first_message_from_a_pipe_and_prints_the_project_id() {
    let run = Run::start();
    run.long_waits();
    let o = pipeline(
        &run,
        "printf 'Hello there' | x100 new - | xargs -I{} \"$X100\" --socket \"$SOCK\" chat --project {} -f --until 'Hello there' --timeout 30",
    );
    assert_eq!(code(&o), 0, "{}", err(&o));
    let line = out(&o);
    let f: Vec<&str> = line.trim_end().split('\t').collect();
    assert_eq!((f[1], f[2]), ("you", "Hello there"));
    let state = run.client.state();
    assert_eq!(state.projects.len(), 1);
    let (_, p) = state.projects.iter().next().unwrap();
    assert!(p.messages().any(|m| m.message.body == "Hello there"));
    // `projects` prints the same project: id, name, agents, live or idle.
    let list = out(&x100(&run, &["projects"]));
    assert_eq!(list.lines().count(), 1);
    let fields: Vec<&str> = list.trim_end().split('\t').collect();
    assert_eq!(fields[0], state.projects.keys().next().unwrap().to_string());
    assert_eq!(fields[1], "Untitled");
    assert_eq!(fields[2], p.agent_count().to_string());
}
