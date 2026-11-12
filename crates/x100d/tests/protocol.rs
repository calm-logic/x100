//! The protocol against a daemon in this process: its limits, the methods one by one, and the
//! jobs clock. Raw sockets where the client library would hide what is being tested.

mod common;

use std::time::Duration;

use common::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use x100_client::{Change, Client, ClientError, Options};
use x100_core::event::ProjectEvent;
use x100_core::id::ProjectId;
use x100_core::message::{Message as ChatMessage, Route};
use x100_core::party::Actor;
use x100_proto::*;

struct Local {
    env: Env,
    handle: x100d::Handle,
    stop: x100d::Stop,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Local {
    async fn start(tweak: impl FnOnce(&mut x100d::Config)) -> Local {
        let env = Env::new();
        let mut config = x100d::Config::new(&env.data);
        config.scripts_dir = Some(env.scripts.clone());
        config.fake_program = Some(env!("CARGO_BIN_EXE_x100d-fake-harness").into());
        tweak(&mut config);
        let daemon = x100d::Daemon::start(config)
            .await
            .expect("the daemon starts");
        let (handle, stop) = (daemon.handle(), daemon.stop_handle());
        let task = tokio::spawn(daemon.run());
        Local {
            env,
            handle,
            stop,
            task: Some(task),
        }
    }

    async fn client(&self) -> Client {
        let mut o = Options::new(self.env.socket(), "test");
        o.startup_timeout = WAIT;
        Client::connect(o).await.unwrap()
    }

    async fn finish(mut self) {
        self.stop.now();
        self.task.take().unwrap().await.unwrap();
    }
}

fn rpc_code(e: ClientError) -> ErrorCode {
    match e {
        ClientError::Rpc(e) => e.code,
        other => panic!("not a protocol error: {other}"),
    }
}

/// A connection with no client library: lines in, lines out.
struct Raw {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    write: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}

impl Raw {
    async fn connect(env: &Env) -> Raw {
        let (r, write) = UnixStream::connect(env.socket())
            .await
            .unwrap()
            .into_split();
        Raw {
            lines: BufReader::new(r).lines(),
            write,
            next: 0,
        }
    }

    async fn send(&mut self, call: impl Into<Call>) -> Id {
        self.next += 1;
        let id = Id::Number(self.next);
        let line = Message::Request(Request {
            id: id.clone(),
            call: call.into(),
        })
        .to_line();
        self.write
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        id
    }

    async fn read(&mut self) -> Option<Message> {
        let line = tokio::time::timeout(WAIT, self.lines.next_line())
            .await
            .expect("a line")
            .unwrap()?;
        Some(Message::parse(&line).expect("the daemon speaks the protocol"))
    }

    /// The response to `id`, collecting the notices before it.
    async fn answer(&mut self, id: &Id) -> (Response, Vec<Notice>) {
        let mut notices = Vec::new();
        loop {
            match self.read().await.expect("still connected") {
                Message::Response(r) if r.id.as_ref() == Some(id) => return (r, notices),
                Message::Notice(n) => notices.push(n),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    async fn call<M: Method + Into<Call>>(&mut self, params: M) -> Result<M::Result, RpcError> {
        let id = self.send(params).await;
        self.answer(&id).await.0.into_result::<M>()
    }

    async fn hello(&mut self) {
        self.call(Hello {
            client: "raw".into(),
            protocol: 1,
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_comes_first_and_names_a_version_the_daemon_speaks() {
    let d = Local::start(|_| {}).await;
    let mut raw = Raw::connect(&d.env).await;
    let early = raw.call(GetDaemon {}).await.unwrap_err();
    assert_eq!(early.code, ErrorCode::InvalidRequest);
    let old = raw
        .call(Hello {
            client: "raw".into(),
            protocol: 0,
        })
        .await
        .unwrap_err();
    assert_eq!(old.code, ErrorCode::UnsupportedProtocol);
    let newer = raw
        .call(Hello {
            client: "raw".into(),
            protocol: 9,
        })
        .await
        .unwrap();
    assert_eq!(newer.protocol, 1, "the version both speak");
    raw.write.write_all(b"not json\n").await.unwrap();
    match raw.read().await.unwrap() {
        Message::Response(r) => assert_eq!(r.outcome.unwrap_err().code, ErrorCode::ParseError),
        other => panic!("{other:?}"),
    }
    raw.write.write_all(b"[]\n").await.unwrap();
    match raw.read().await.unwrap() {
        Message::Response(r) => assert_eq!(r.outcome.unwrap_err().code, ErrorCode::InvalidRequest),
        other => panic!("{other:?}"),
    }
    d.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_line_over_the_limit_is_refused_and_ends_the_connection() {
    let d = Local::start(|c| c.max_line = 4096).await;
    let mut raw = Raw::connect(&d.env).await;
    raw.hello().await;
    // Under the limit is fine.
    let ok = raw
        .call(UploadFile {
            project: ProjectId::from_u128(1),
            name: "a".into(),
            media_type: "text/plain".into(),
            data: Bytes(vec![1; 1000]),
        })
        .await;
    assert_eq!(
        ok.unwrap_err().code,
        ErrorCode::NotFound,
        "read and understood"
    );
    raw.write.write_all(&vec![b'x'; 10_000]).await.unwrap();
    raw.write.write_all(b"\n").await.unwrap();
    match raw.read().await.unwrap() {
        Message::Response(r) => assert_eq!(r.outcome.unwrap_err().code, ErrorCode::TooLarge),
        other => panic!("{other:?}"),
    }
    assert!(raw.read().await.is_none(), "the connection is closed");
    // Another connection is unaffected.
    let mut other = Raw::connect(&d.env).await;
    other.hello().await;
    d.finish().await;
}

fn big(body: usize) -> ChatMessage {
    ChatMessage {
        id: x100_core::id::MessageId::from_u128(rand_id()),
        route: Route::BuilderToCeo,
        body: "x".repeat(body),
        attachments: vec![],
        in_reply_to: None,
    }
}

fn rand_id() -> u128 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    u128::from(NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)) << 64 | 7
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscriber_that_lags_is_cut_off_and_resumes_without_a_gap() {
    let d = Local::start(|c| {
        c.queue_capacity = 8;
        c.lag_after = Duration::from_millis(300);
    })
    .await;
    let client = d.client().await;
    let project = create_project(&client, &d.env, "lag").await;
    let log = d.handle.project_log(project).unwrap();

    let mut raw = Raw::connect(&d.env).await;
    raw.hello().await;
    let sub = raw.call(Subscribe { resume: None }).await.unwrap();
    let mut state = sub.snapshot.unwrap().projects.remove(&project).unwrap();
    // The client reads nothing while a few megabytes of events pass: far more than the socket
    // buffers and the queue hold.
    for _ in 0..100 {
        let log = log.clone();
        tokio::task::spawn_blocking(move || {
            log.append(Actor::Builder, ProjectEvent::MessageSent(big(200_000)))
                .unwrap()
        })
        .await
        .unwrap();
    }
    let last = log.last().unwrap();
    // Long enough for the full queue to pass the limit and the cut to be made.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut applied = state.seq().unwrap();
    let closed = loop {
        match raw.read().await.expect("still connected") {
            Message::Notice(Notice::Event(EventNotice::Project { envelope, .. })) => {
                state
                    .apply(&envelope)
                    .expect("in order, none missed before the cut");
                applied = envelope.seq;
            }
            Message::Notice(Notice::Closed(c)) => break c,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(closed.because, ClosedBecause::Lagged);
    assert_eq!(closed.subscription, sub.subscription);
    assert!(applied < last, "it was cut off before the end");

    // Subscribing again with a cursor gives the rest, once, on the same connection.
    let cursor = Cursor {
        daemon: None,
        projects: [(project, applied)].into(),
    };
    let again = raw
        .call(Subscribe {
            resume: Some(cursor),
        })
        .await
        .unwrap();
    assert!(again.snapshot.is_none());
    while applied < last {
        match raw.read().await.expect("still connected") {
            Message::Notice(Notice::Event(EventNotice::Project {
                subscription,
                envelope,
                ..
            })) => {
                assert_eq!(subscription, again.subscription);
                state.apply(&envelope).expect("no gap and no repeat");
                applied = envelope.seq;
            }
            Message::Notice(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(state, log.state());
    d.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_created_later_is_followed_from_its_first_event() {
    let d = Local::start(|_| {}).await;
    let client = d.client().await;
    let mut changes = client.changes();
    let project = create_project(&client, &d.env, "late").await;
    // The client had no state for it, and applied its log from `Created`: no fold error, so the
    // state was never replaced.
    assert_eq!(
        client.project(project).unwrap().seq().map(|s| s.0 >= 1),
        Some(true)
    );
    while let Ok(c) = changes.try_recv() {
        assert_ne!(c, Change::Reset);
    }
    // A cursor that does not name the project reads it from the start.
    let mut raw = Raw::connect(&d.env).await;
    raw.hello().await;
    let id = raw
        .send(Subscribe {
            resume: Some(Cursor::default()),
        })
        .await;
    let (_, _) = raw.answer(&id).await;
    let mut seen = Vec::new();
    while seen.is_empty() {
        if let Message::Notice(Notice::Event(EventNotice::Project { envelope, .. })) =
            raw.read().await.unwrap()
        {
            seen.push(envelope.seq.0);
        }
    }
    assert_eq!(seen[0], 1);
    d.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn methods_that_later_milestones_make_real_say_which() {
    let d = Local::start(|_| {}).await;
    let client = d.client().await;
    let (p, a) = (
        ProjectId::from_u128(1),
        x100_core::id::AgentId::from_u128(1),
    );
    let later: Vec<(Call, &str)> = vec![
        (
            ReplaceDriver {
                project: p,
                agent: a,
            }
            .into(),
            "M9",
        ),
        (
            RemoveCredential {
                credential: x100_core::id::CredentialRef::from_u128(1),
            }
            .into(),
            "M11",
        ),
        (
            ReplaceCredential {
                credential: x100_core::id::CredentialRef::from_u128(1),
                value: Secret::new("v"),
            }
            .into(),
            "M11",
        ),
        (
            ScopeCredential {
                credential: x100_core::id::CredentialRef::from_u128(1),
                scope: x100_core::credential::CredentialScope {
                    hosts: Default::default(),
                    commands: Default::default(),
                    projects: x100_core::credential::Projects::All,
                },
            }
            .into(),
            "M11",
        ),
        (
            AddCredential {
                name: "n".into(),
                value: Secret::new("v"),
                scope: x100_core::credential::CredentialScope {
                    hosts: Default::default(),
                    commands: Default::default(),
                    projects: x100_core::credential::Projects::All,
                },
            }
            .into(),
            "M11",
        ),
        (
            DecideAction {
                project: p,
                action: x100_core::id::ActionId::from_u128(1),
                decision: x100_core::action::Decision::Approved,
            }
            .into(),
            "M12",
        ),
        (
            ListMemories {
                project: p,
                agent: a,
            }
            .into(),
            "M13",
        ),
        (
            ListSkills {
                project: p,
                scope: None,
            }
            .into(),
            "M13",
        ),
        (Reflect { project: p }.into(), "M14"),
        (ListTrials { project: p }.into(), "M14"),
    ];
    let mut raw = Raw::connect(&d.env).await;
    raw.hello().await;
    let mut later_names = Vec::new();
    for (call, milestone) in later {
        let name = call.method();
        later_names.push(name);
        let id = raw.send(call).await;
        let (r, _) = raw.answer(&id).await;
        let e = r.outcome.unwrap_err();
        assert_eq!(e.code, ErrorCode::Unavailable, "{name}");
        assert!(
            e.message.contains(milestone) && e.message.contains(name),
            "{}",
            e.message
        );
    }
    // Every method is real or in the list above, so a milestone that adds one has to say so here.
    let real = [
        "hello",
        "events.subscribe",
        "unsubscribe",
        "events.read",
        "transcript.read",
        "transcript.subscribe",
        "terminal.read",
        "terminal.subscribe",
        "daemon.get",
        "daemon.set_setting",
        "projects.list",
        "projects.get",
        "projects.create",
        "projects.set",
        "chat.brief",
        "chat.send",
        "tasks.raise",
        "files.upload",
        "files.read",
        "files.remove",
        "jobs.cancel",
        "agents.inbox",
        "agents.waits",
        "drivers.start_pack",
        "drivers.stop",
        "daemon.set_clock",
        "machines.set_cap",
        "harnesses.rescan",
        "agents.set_budget",
        "agents.set_effort",
    ];
    let mut named: Vec<&str> = real.to_vec();
    named.extend(later_names);
    named.sort_unstable();
    let mut all: Vec<&str> = METHODS.iter().map(|m| m.name).collect();
    all.sort_unstable();
    assert_eq!(named, all);
    let _ = client;
    d.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_files_tasks_and_settings_work_through_the_protocol() {
    use x100_core::project::SettingChange;
    let d = Local::start(|_| {}).await;
    let c = d.client().await;

    // Creating: a relative path and a path twice are refused.
    let bad = c
        .request(CreateProject {
            path: "relative".into(),
            name: None,
            settings: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(bad), ErrorCode::Rejected);
    let project = create_project(&c, &d.env, "plants").await;
    let again = c
        .request(CreateProject {
            path: d.env.project_dir("plants"),
            name: None,
            settings: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(again), ErrorCode::Rejected);

    let list = c.request(ListProjects {}).await.unwrap();
    assert_eq!(list.projects.len(), 1);
    assert_eq!(
        (
            list.projects[0].name.as_str(),
            list.projects[0].agents,
            list.projects[0].live
        ),
        ("plants", 1, false)
    );
    let got = c.request(GetProject { project }).await.unwrap();
    // The client follows the log on its own, so compare once it has applied what the daemon
    // answered from.
    c.wait_for(WAIT, |s| {
        (s.projects.get(&project)?.seq() == got.seq()).then_some(())
    })
    .await
    .expect("the client reaches the daemon's answer");
    assert_eq!(got, c.project(project).unwrap());
    let missing = c
        .request(GetProject {
            project: ProjectId::from_u128(9),
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(missing), ErrorCode::NotFound);
    assert!(
        std::path::Path::new(&d.env.project_dir("plants"))
            .join(".git")
            .exists()
    );

    // Settings of the project and of the daemon.
    c.request(ChangeProjectSetting {
        project,
        change: SettingChange::Budget(x100_core::money::Money::from_dollars(7)),
    })
    .await
    .unwrap();
    c.wait_for(WAIT, |s| {
        (s.projects[&project].project()?.settings.budget
            == x100_core::money::Money::from_dollars(7))
        .then_some(())
    })
    .await
    .unwrap();
    let zero = c
        .request(SetDaemonSetting {
            setting: x100_core::settings::DaemonSetting::StallTimeout(Duration::ZERO),
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(zero), ErrorCode::Rejected);
    c.request(SetDaemonSetting {
        setting: x100_core::settings::DaemonSetting::StallTimeout(Duration::from_secs(90)),
    })
    .await
    .unwrap();
    let daemon = c.request(GetDaemon {}).await.unwrap();
    assert_eq!(daemon.settings().stall_timeout, Duration::from_secs(90));
    c.wait_for(WAIT, |s| {
        (s.daemon.settings().stall_timeout == Duration::from_secs(90)).then_some(())
    })
    .await
    .unwrap();

    // Files: upload, read, attach to the brief, remove.
    let up = c
        .request(UploadFile {
            project,
            name: "sketch.png".into(),
            media_type: "image/png".into(),
            data: Bytes(vec![1, 2, 3]),
        })
        .await
        .unwrap();
    assert_eq!((up.file.size, up.file.attached_to), (3, None));
    let read = c
        .request(ReadFile {
            project,
            file: up.file.id,
        })
        .await
        .unwrap();
    assert_eq!(read.data, Bytes(vec![1, 2, 3]));
    let ghost = x100_core::id::FileId::from_u128(5);
    let no = c
        .request(SendBrief {
            project,
            body: "b".into(),
            attachments: vec![ghost],
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(no), ErrorCode::NotFound);
    let brief = c
        .request(SendBrief {
            project,
            body: "A site for plants.\nWith a waitlist.".into(),
            attachments: vec![up.file.id],
        })
        .await
        .unwrap();
    let twice = c
        .request(SendBrief {
            project,
            body: "again".into(),
            attachments: vec![],
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(twice), ErrorCode::WrongState);
    // The brief is delegated to the CEO as a task, and the CEO answers it.
    let s = c
        .wait_for(WAIT, |s| {
            let p = &s.projects[&project];
            p.messages()
                .any(|m| m.message.route == Route::CeoToBuilder)
                .then(|| p.clone())
        })
        .await
        .unwrap();
    let task = s.task(brief.task).unwrap();
    assert_eq!(task.task.title, "A site for plants.");
    assert!(
        task.state
            .assignment()
            .is_some_and(|a| a.assignee == s.project().unwrap().ceo)
    );
    let reply = s
        .messages()
        .find(|m| m.message.route == Route::CeoToBuilder)
        .unwrap();
    assert!(
        reply.message.body.contains("Attached: sketch.png"),
        "{}",
        reply.message.body
    );

    c.request(RemoveFile {
        project,
        file: up.file.id,
    })
    .await
    .unwrap();
    let gone = c
        .request(ReadFile {
            project,
            file: up.file.id,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(gone), ErrorCode::NotFound);

    // A raised task is nobody's yet.
    let raised = c
        .request(RaiseTask {
            project,
            title: "Dark mode".into(),
            ask: "Add it.".into(),
            part: None,
            attachments: vec![],
        })
        .await
        .unwrap();
    c.wait_for(WAIT, |s| s.projects[&project].task(raised.task).map(|_| ()))
        .await
        .unwrap();

    // The log reads back a page at a time, newest first.
    let page = c
        .request(ReadLog {
            log: LogId::Project(project),
            page: Page::NewestFirst {
                before: None,
                limit: 3,
            },
        })
        .await
        .unwrap();
    let LogPage::Project { events } = page else {
        panic!()
    };
    assert_eq!(events.len(), 3);
    assert!(events[0].seq > events[1].seq);
    let daemon_page = c
        .request(ReadLog {
            log: LogId::Daemon,
            page: Page::OldestFirst {
                after: None,
                limit: 1000,
            },
        })
        .await
        .unwrap();
    let LogPage::Daemon { events } = daemon_page else {
        panic!()
    };
    assert!(events.len() >= 3, "harness, machine, project and settings");

    let ceo = s.project().unwrap().ceo;
    let inbox = c
        .request(ReadInbox {
            project,
            agent: ceo,
        })
        .await
        .unwrap();
    assert!(inbox.items.len() <= 1);

    // The CEO's driver started from a start pack, which reads back as it was given.
    let driver = s.agent(ceo).unwrap().drivers[0];
    let pack = c.request(ReadStartPack { project, driver }).await.unwrap();
    assert!(pack.text.starts_with("ROLE: CEO\n"), "{}", pack.text);
    assert!(pack.text.contains("A site for plants."), "{}", pack.text);
    let waits = c
        .request(ReadWaits {
            project,
            agent: ceo,
        })
        .await
        .unwrap();
    assert!(waits.waits.is_empty(), "the fake's CEO never waits");
    let no_pack = c
        .request(ReadStartPack {
            project,
            driver: x100_core::id::DriverId::from_u128(9),
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(no_pack), ErrorCode::NotFound);
    d.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_wakes_an_agent_fires_and_a_cancelled_one_does_not() {
    use x100_core::job::{ProjectJob, Schedule};
    let d = Local::start(|_| {}).await;
    let c = d.client().await;
    let project = create_project(&c, &d.env, "jobs").await;
    let ceo = c.with_state(|s| s.projects[&project].project().unwrap().ceo);
    let log = d.handle.project_log(project).unwrap();
    let scheduler = d.handle.scheduler(project).unwrap();
    let _ = &log;
    let job = |note: &str| ProjectJob::Wake {
        scheduled_by: ceo,
        agent: ceo,
        note: note.into(),
    };
    let min = x100_core::job::Interval::new(Duration::from_secs(1)).unwrap();

    let later = {
        let (s, j) = (scheduler.clone(), job("never"));
        tokio::task::spawn_blocking(move || {
            s.schedule_after(Actor::Agent(ceo), Duration::from_secs(3600), j, min)
                .unwrap()
        })
        .await
        .unwrap()
    };
    c.request(CancelJob {
        project,
        job: later,
    })
    .await
    .unwrap();
    let twice = c
        .request(CancelJob {
            project,
            job: later,
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(twice), ErrorCode::WrongState);
    let none = c
        .request(CancelJob {
            project,
            job: x100_core::id::JobId::from_u128(3),
        })
        .await
        .unwrap_err();
    assert_eq!(rpc_code(none), ErrorCode::NotFound);

    let now = x100_jobs::SystemClock;
    let due = {
        let (s, j) = (scheduler.clone(), job("time to check the site"));
        tokio::task::spawn_blocking(move || {
            use x100_core::time::Clock;
            s.schedule(Actor::Agent(ceo), Schedule::Once { at: now.now() }, j, min)
                .unwrap()
        })
        .await
        .unwrap()
    };
    // Firing delivers the note to the CEO, whose driver answers it.
    let reply = c
        .wait_for(WAIT, |s| {
            s.projects[&project]
                .messages()
                .find(|m| m.message.route == Route::CeoToBuilder)
                .cloned()
        })
        .await
        .unwrap();
    assert!(
        reply
            .message
            .body
            .contains("A scheduled job woke you: time to check the site")
    );
    let fired = c.project(project).unwrap().job(due).unwrap().fired;
    assert_eq!(fired, 1);
    assert_eq!(c.project(project).unwrap().job(later).unwrap().fired, 0);
    d.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_builders_controls_for_drivers_work_through_the_protocol() {
    let d = Local::start(|_| {}).await;
    let c = d.client().await;
    let project = create_project(&c, &d.env, "plants").await;
    let code = |r: Result<Done, ClientError>| rpc_code(r.unwrap_err());

    // A machine's cap: known machines only.
    let machine = c.with_state(|s| s.daemon.machines().next().unwrap().id);
    c.request(SetMachineCap { machine, cap: 3 }).await.unwrap();
    c.wait_for(WAIT, |s| {
        (s.daemon.machines().next()?.cap == 3).then_some(())
    })
    .await
    .expect("the cap reaches the client");
    let unknown = x100_core::id::MachineId::from_u128(999);
    assert_eq!(
        code(
            c.request(SetMachineCap {
                machine: unknown,
                cap: 3
            })
            .await
        ),
        ErrorCode::NotFound
    );

    // A rescan finds the same harnesses again, under the ids the log already gave them.
    let known: Vec<_> = c.with_state(|s| s.daemon.harnesses().map(|h| h.id).collect());
    let found = c.request(RescanHarnesses {}).await.unwrap();
    assert_eq!(found.harnesses, known);

    // An effort range for the CEO, and a budget only for a lead, inside the project's total.
    let ceo = c.with_state(|s| s.projects[&project].project().unwrap().ceo);
    let range = x100_core::effort::EffortRange::new(
        x100_core::effort::Effort::Low,
        x100_core::effort::Effort::High,
    )
    .unwrap();
    c.request(SetEffortRange {
        project,
        agent: ceo,
        range,
    })
    .await
    .unwrap();
    c.wait_for(WAIT, |s| {
        let team = &s.projects.get(&project)?.project()?.team;
        (team.effort(ceo) == Some(range)).then_some(())
    })
    .await
    .expect("the range reaches the client");
    let budget = x100_core::money::Money::from_dollars(1);
    assert_eq!(
        code(
            c.request(SetLeadBudget {
                project,
                lead: ceo,
                budget
            })
            .await
        ),
        ErrorCode::Rejected,
        "the CEO has no budget of its own"
    );
    let nobody = x100_core::id::AgentId::from_u128(999);
    assert_eq!(
        code(
            c.request(SetLeadBudget {
                project,
                lead: nobody,
                budget
            })
            .await
        ),
        ErrorCode::NotFound
    );
    d.finish().await;
}
