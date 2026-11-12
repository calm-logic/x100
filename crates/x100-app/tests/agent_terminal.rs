//! An agent's view of terminal drivers, against a scripted daemon on a socket. The state is core's
//! Fernleaf with Research's drivers made terminal drivers. Driver 1's terminal is a real `claude`
//! recording; for driver 2 the daemon says it has no terminal. `real_terminal.rs` runs the same
//! view against the real daemon.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use gpui::{TestAppContext, VisualTestContext, prelude::*};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use x100_app::session::Session;
use x100_app::team::AgentOutput;
use x100_app::term::{FONT, SETTLE, Size, screen_text};
use x100_client::{Client, Options};
use x100_core::driver::{Purpose, View};
use x100_core::event::ProjectEvent;
use x100_core::fold::{DaemonState, ProjectState};
use x100_core::id::{AgentId, DriverId};
use x100_core::terminal::{Chunk, ChunkKind, TermPos};
use x100_core::testing::{FERNLEAF, START, daemon_log, fernleaf};
use x100_core::time::Timestamp;
use x100_core::transcript::{Entry, Item, Pos};
use x100_proto::{
    Bytes, Call, ErrorCode, Id, Message, Notice, Response, RpcError, Snapshot, SubscriptionId,
    TerminalNotice, TranscriptNotice,
};

const CLAUDE: &[u8] = include_bytes!("data/claude.raw");
const CLAUDE_TIMING: &str = include_str!("data/claude.timing");
const WAIT: Duration = Duration::from_secs(20);

struct World {
    snapshot: Snapshot,
    /// Both of its drivers have ended.
    research: AgentId,
    drivers: [DriverId; 2],
    /// An agent whose driver is still live, and that driver.
    live: (AgentId, DriverId),
}

fn world() -> World {
    let mut f = fernleaf();
    let folded = ProjectState::fold(&f.log).expect("the log folds");
    let live = folded
        .live_drivers()
        .find(|d| d.driver.purpose == Purpose::Work)
        .map(|d| (d.driver.agent, d.driver.id))
        .expect("a live driver at the end of the run");
    for e in &mut f.log {
        if let ProjectEvent::DriverStarted(d) = &mut e.event
            && (d.agent == f.research || d.id == live.1)
        {
            d.view = View::Terminal;
        }
    }
    let project = ProjectState::fold(&f.log).expect("the log folds");
    let daemon = DaemonState::fold(&daemon_log(START)).expect("the daemon's log folds");
    let had = project.agent(f.research).unwrap().drivers.clone();
    assert!(had.len() >= 2, "{had:?}");
    assert!(
        had.iter()
            .all(|d| !project.driver(*d).unwrap().state.is_live())
    );
    World {
        snapshot: Snapshot {
            daemon,
            projects: BTreeMap::from([(FERNLEAF, project)]),
        },
        research: f.research,
        drivers: [had[0], had[1]],
        live,
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

fn entry(driver: DriverId, item: Item) -> Entry {
    Entry {
        at: Timestamp::from_millis(START.as_millis() + 1),
        driver,
        item,
    }
}

/// What the agent's transcript holds: each driver's start and one line of text.
fn transcript(d: [DriverId; 2]) -> Vec<Entry> {
    let pack = |d| {
        entry(
            d,
            Item::StartPack {
                text: "ROLE: Research\n".into(),
            },
        )
    };
    vec![
        pack(d[0]),
        entry(d[0], Item::DriverChanged { from: None }),
        entry(
            d[0],
            Item::Text {
                text: "Shown on the screen, not in a log.".into(),
            },
        ),
        pack(d[1]),
        entry(d[1], Item::DriverChanged { from: Some(d[0]) }),
        entry(
            d[1],
            Item::Text {
                text: "Two of three price per seat.".into(),
            },
        ),
    ]
}

/// The recording as the daemon would store it: its size, then each read the program made.
fn chunks() -> Vec<Chunk> {
    let mut out = vec![Chunk {
        at: START,
        what: ChunkKind::Resize {
            cols: 120,
            rows: 40,
        },
    }];
    let mut at = 0;
    for n in CLAUDE_TIMING
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.parse::<usize>().ok())
    {
        out.push(Chunk {
            at: START,
            what: ChunkKind::Output {
                bytes: Bytes(CLAUDE[at..at + n].to_vec()),
            },
        });
        at += n;
    }
    out
}

/// Entries the test hands the daemon to send later, on the transcript subscription.
type More = Arc<Mutex<Option<mpsc::UnboundedReceiver<Entry>>>>;

fn transcript_notice(position: u64, entry: Entry) -> String {
    Message::Notice(Notice::TranscriptEntry(TranscriptNotice {
        subscription: SubscriptionId(5),
        position: Pos(position),
        entry,
    }))
    .to_line()
}

/// Every method the view asked for, and each terminal size with its driver.
#[derive(Clone, Default)]
struct Asked {
    methods: Arc<Mutex<Vec<String>>>,
    resized: Arc<Mutex<Vec<(DriverId, u16, u16)>>>,
}

impl Asked {
    fn methods(&self) -> Vec<String> {
        self.methods.lock().unwrap().clone()
    }

    fn resized(&self) -> Vec<(DriverId, u16, u16)> {
        self.resized.lock().unwrap().clone()
    }
}

/// The daemon's side. Every method asked for is recorded.
async fn serve(listener: UnixListener, w: Arc<World>, asked: Asked, more: More) {
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
            asked
                .methods
                .lock()
                .unwrap()
                .push(request.call.method().to_owned());
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
                Call::SubscribeTranscript(s) if s.agent == w.research => {
                    send(ok(id, serde_json::json!({"subscription": 5})));
                    let entries = transcript(w.drivers);
                    let mut position = entries.len() as u64;
                    for (i, entry) in entries.into_iter().enumerate() {
                        send(transcript_notice(i as u64 + 1, entry));
                    }
                    if let Some(mut later) = more.lock().unwrap().take() {
                        let out = out.clone();
                        tokio::spawn(async move {
                            while let Some(entry) = later.recv().await {
                                position += 1;
                                let _ = out.send(transcript_notice(position, entry));
                            }
                        });
                    }
                }
                Call::SubscribeTranscript(s) if s.agent == w.live.0 => {
                    send(ok(id, serde_json::json!({"subscription": 6})));
                    let started = [
                        Item::StartPack {
                            text: "ROLE: Live\n".into(),
                        },
                        Item::DriverChanged { from: None },
                    ];
                    for (i, item) in started.into_iter().enumerate() {
                        let notice = TranscriptNotice {
                            subscription: SubscriptionId(6),
                            position: Pos(i as u64 + 1),
                            entry: entry(w.live.1, item),
                        };
                        send(Message::Notice(Notice::TranscriptEntry(notice)).to_line());
                    }
                }
                Call::ResizeTerminal(r) => {
                    asked
                        .resized
                        .lock()
                        .unwrap()
                        .push((r.driver, r.cols, r.rows));
                    send(ok(id, serde_json::json!({})));
                }
                Call::SubscribeTerminal(s) if s.driver == w.drivers[0] || s.driver == w.live.1 => {
                    send(ok(id, serde_json::json!({"subscription": 7})));
                    for (i, chunk) in chunks().into_iter().enumerate() {
                        send(
                            Message::Notice(Notice::TerminalChunk(TerminalNotice {
                                subscription: SubscriptionId(7),
                                position: TermPos(i as u64 + 1),
                                chunk,
                            }))
                            .to_line(),
                        );
                    }
                }
                Call::SubscribeTerminal(s) => send(err(
                    id,
                    ErrorCode::NotFound,
                    &format!("driver {} runs headless and has no terminal", s.driver),
                )),
                Call::Unsubscribe(_) => send(ok(id, serde_json::json!({}))),
                _ => send(err(id, ErrorCode::NotFound, "not scripted")),
            }
        }
    }
}

fn connect(
    rt: &tokio::runtime::Runtime,
    dir: &tempfile::TempDir,
    w: &Arc<World>,
    asked: &Asked,
    more: More,
) -> Client {
    let socket = dir.path().join("x100d.sock");
    rt.block_on(async {
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(serve(listener, Arc::clone(w), asked.clone(), more));
        Client::connect(Options::new(&socket, "test"))
            .await
            .unwrap()
    })
}

#[test]
fn terminal_drivers_replay_and_a_refused_one_says_so() {
    // As the window tests: a plain test, since the client wakes GPUI's tasks from Tokio's threads.
    let mut cx = TestAppContext::single();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let w = Arc::new(world());
    let asked = Asked::default();
    let (later, rx) = mpsc::unbounded_channel();
    let more: More = Arc::new(Mutex::new(Some(rx)));
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&rt, &dir, &w, &asked, more);
    let research = w.research;
    let (output, vcx) = cx.add_window_view(move |_, cx| {
        let session = cx.new(|cx| Session::new(client, cx));
        AgentOutput::new(session, FERNLEAF, research, cx)
    });
    let vcx = vcx.clone();

    // The recording, replayed into a bare emulator: what driver 1's screen must show.
    let mut want = Term::new(
        Config::default(),
        &Size {
            cols: 120,
            rows: 40,
        },
        VoidListener,
    );
    let mut parser: Processor = Processor::new();
    parser.advance(&mut want, CLAUDE);
    let want = screen_text(&want);

    let until = Instant::now() + WAIT;
    let shown = loop {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        let shown = vcx.read(|cx| output.read(cx).shown(cx));
        if shown.len() == 2
            && shown[0].screen == want
            && shown[1].text.contains("Two of three price per seat.")
        {
            break shown;
        }
        assert!(
            Instant::now() < until,
            "the view did not settle: {shown:#?}"
        );
    };

    // Driver 1: its own screen, replayed at its recorded size, its transcript not repeated.
    assert_eq!(shown[0].terminal_of, Some(w.drivers[0]));
    let first = shown[0].marker.as_deref().unwrap();
    assert!(first.starts_with("Driver 1 started: "), "{first}");
    assert!(!shown[0].text.contains("Shown on the screen"));
    // Driver 2: the change marked with why driver 1 ended, a plain line for the terminal the
    // daemon will not show, and its transcript in its place.
    let second = shown[1].marker.as_deref().unwrap();
    assert!(second.starts_with("Driver 2 took over: "), "{second}");
    assert!(second.contains("Driver 1 ended: "), "{second}");
    assert!(
        shown[1]
            .text
            .contains("This driver has no terminal to show. Its transcript follows.")
    );
    assert!(!shown[1].text.contains("Shown on the screen"));

    // Nothing blinks: a tool's output arriving in driver 2's log redraws that log, and not
    // driver 1's screen. The views that hold the log (this one up to the window's
    // root) run their render again, as GPUI does for every ancestor of a changed view, and reuse
    // their cached children as they were.
    let views = vcx.read(|cx| output.read(cx).terminals());
    let counts = |vcx: &VisualTestContext| {
        vcx.read(|cx| {
            let o = output.read(cx);
            (
                o.renders(),
                views[0].read(cx).stats().renders,
                views[1].read(cx).stats().renders,
            )
        })
    };
    let before = counts(&vcx);
    for i in 0..20 {
        later
            .send(entry(
                w.drivers[1],
                Item::ToolOutput {
                    call: "c9".into(),
                    chunk: format!("checked shop {i}\n"),
                },
            ))
            .unwrap();
    }
    let until = Instant::now() + WAIT;
    while !vcx.read(|cx| {
        output.read(cx).shown(cx)[1]
            .text
            .contains("checked shop 19")
    }) {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        assert!(Instant::now() < until, "the tool's output did not arrive");
    }
    let after = counts(&vcx);
    assert_eq!(after.1, before.1, "{before:?} {after:?}");
    assert!(after.2 > before.2, "{before:?} {after:?}");

    // Watching only: both drivers have ended, so however long their panes have held a size,
    // the view asked to follow things and asked for nothing else.
    vcx.executor().advance_clock(SETTLE * 4);
    vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(50));
    vcx.run_until_parked();
    assert!(
        vcx.read(|cx| views[0].read(cx).pane()).is_some(),
        "its pane was measured"
    );
    assert_eq!(asked.resized(), []);
    let asked = asked.methods();
    for method in &asked {
        assert!(
            [
                "hello",
                "events.subscribe",
                "transcript.subscribe",
                "terminal.subscribe"
            ]
            .contains(&method.as_str()),
            "{asked:?}"
        );
    }
    assert_eq!(
        asked.iter().filter(|m| *m == "terminal.subscribe").count(),
        2
    );
    drop(rt);
}

#[test]
fn a_live_drivers_terminal_is_asked_to_take_the_panes_size_once_and_again_when_it_changes() {
    let mut cx = TestAppContext::single();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let w = Arc::new(world());
    let asked = Asked::default();
    let more: More = Arc::new(Mutex::new(None));
    let dir = tempfile::tempdir().unwrap();
    let client = connect(&rt, &dir, &w, &asked, more);
    let (agent, driver) = w.live;
    let (output, vcx) = cx.add_window_view(move |_, cx| {
        let session = cx.new(|cx| Session::new(client, cx));
        AgentOutput::new(session, FERNLEAF, agent, cx)
    });
    let mut vcx = vcx.clone();
    vcx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(700.)));

    // Until its screen is drawn and its pane has held a size, nothing is asked.
    let until = Instant::now() + WAIT;
    let screen = loop {
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        let shown = vcx.read(|cx| output.read(cx).shown(cx));
        if shown.len() == 1 && shown[0].screen.iter().any(|l| !l.trim().is_empty()) {
            break vcx.read(|cx| output.read(cx).terminals()[0].clone());
        }
        assert!(
            Instant::now() < until,
            "the screen was never drawn: {shown:#?}"
        );
    };
    assert_eq!(vcx.read(|cx| screen.read(cx).font_size()), Some(FONT));
    assert_eq!(asked.resized(), []);

    // Once the pane's size holds, the daemon is asked once for it.
    let resized = |vcx: &mut VisualTestContext, n: usize| {
        let until = Instant::now() + WAIT;
        loop {
            vcx.executor().advance_clock(SETTLE);
            vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(5));
            let got = asked.resized();
            if got.len() >= n {
                return got;
            }
            assert!(Instant::now() < until, "asked {got:?}");
        }
    };
    let first = resized(&mut vcx, 1);
    let pane = vcx.read(|cx| screen.read(cx).pane()).unwrap();
    assert_eq!(first, [(driver, pane.cols, pane.rows)]);
    // More frames and time ask nothing more.
    for _ in 0..3 {
        vcx.update(|window, _| window.refresh());
        vcx.executor().advance_clock(SETTLE * 2);
        vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(asked.resized().len(), 1);

    // A smaller window: asked again, once, for the new size.
    vcx.simulate_resize(gpui::size(gpui::px(700.), gpui::px(500.)));
    let second = resized(&mut vcx, 2);
    let smaller = vcx.read(|cx| screen.read(cx).pane()).unwrap();
    assert_eq!(second[1], (driver, smaller.cols, smaller.rows));
    assert!(smaller.cols < pane.cols && smaller.rows < pane.rows);
    vcx.executor().advance_clock(SETTLE * 4);
    vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(asked.resized().len(), 2);
    drop(rt);
}
