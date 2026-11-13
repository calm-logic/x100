//! The real `x100` binary in a pseudo-terminal against a real daemon on the fake clock: the first
//! run, the tabs, an agent three levels down, quitting, and running it again to find the same
//! project open. Its screen is read back through a terminal emulator.
//!
//! With `X100_SCREENS=<dir>` set, text captures of a few screens are saved there.

mod common;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const ROWS: u16 = 40;
const COLS: u16 = 130;

struct Pty {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
    // Keeps the master side open for as long as the program runs.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Pty {
    fn spawn(run: &Run) -> Pty {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_x100"));
        cmd.arg("--socket");
        cmd.arg(run.daemon.socket());
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).expect("x100 starts");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("a reader");
        let writer = pair.master.take_writer().expect("a writer");
        let screen = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        let fed = Arc::clone(&screen);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    return;
                }
                fed.lock().unwrap().process(&buf[..n]);
            }
        });
        Pty {
            child,
            writer,
            screen,
            _master: pair.master,
        }
    }

    fn text(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    fn send(&mut self, bytes: &str) {
        self.writer
            .write_all(bytes.as_bytes())
            .expect("keys are written");
        self.writer.flush().unwrap();
    }

    fn wait_for(&self, what: &str, mut done: impl FnMut(&str) -> bool) -> String {
        let until = Instant::now() + WAIT;
        loop {
            let text = self.text();
            if done(&text) {
                return text;
            }
            assert!(
                Instant::now() < until,
                "timed out waiting until {what}; the screen shows\n{text}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Ctrl-C, and the exit status of the program.
    fn quit(&mut self) -> portable_pty::ExitStatus {
        self.send("\x03");
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().expect("waits") {
                return status;
            }
            assert!(
                Instant::now() < until,
                "x100 did not quit; the screen shows\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Opens the agent listed under `name` on the screen: Down to its row, then Enter. A page opens on
/// its first row.
fn open_row(x: &mut Pty, name: &str) {
    let text = x.text();
    let rows: Vec<&str> = text
        .lines()
        .filter(|l| l.trim_start().starts_with(['●', '○', '◐']))
        .collect();
    let at = rows
        .iter()
        // Its state, its face, then its role.
        .position(|l| l.split_whitespace().nth(2) == Some(name))
        .unwrap_or_else(|| panic!("{name} is not listed on\n{text}"));
    x.send(&"\x1b[B".repeat(at));
    x.send("\r");
}

fn capture(name: &str, text: &str) {
    if let Some(dir) = std::env::var_os("X100_SCREENS") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Cells the interface cleared are blanks to the terminal; the saved text has none at ends.
        let text: Vec<&str> = text.lines().map(str::trim_end).collect();
        std::fs::write(dir.join(format!("{name}.txt")), text.join("\n") + "\n").unwrap();
    }
}

#[test]
fn the_full_screen_runs_in_a_terminal_and_the_next_run_opens_the_same_project() {
    let run = Run::start();
    run.long_waits();
    let mut x = Pty::spawn(&run);

    // The first run: a box to write in, no form.
    let text = x.wait_for("the first screen", |s| {
        s.contains("Tell the CEO what you want to build.")
    });
    capture("first-run", &text);
    x.send("Launch Fernleaf\nA landing page for Fernleaf, a plant shop, with a waitlist.\r");
    let text = x.wait_for("the CEO's answer", |s| {
        s.contains("x100 · Fernleaf") && s.contains("Setting it up now.")
    });
    assert!(text.contains("You    Launch Fernleaf"), "{text}");
    let project = *run
        .client
        .state()
        .projects
        .keys()
        .next()
        .expect("the daemon holds a project");
    run.settle(project);
    let text = x.wait_for("the question is asked", |s| {
        s.contains("Forest greens or ocean blues?  (asks you)")
    });
    capture("chat", &text);

    // The tabs, by key: Tab leaves the message box, and digits switch tabs from there.
    x.send("\t\t");
    let text = x.wait_for("the Team tab", |s| s.contains("Enter opens an agent."));
    let p = run.client.project(project).unwrap();
    for a in p.agents() {
        assert!(text.contains(&a.role), "{} is not on\n{text}", a.role);
    }
    capture("team", &text);
    x.send("4");
    let text = x.wait_for("the Specs tab", |s| s.contains("Business requirements"));
    capture("specs", &text);
    x.send("5");
    let text = x.wait_for("the Tasks tab", |s| s.contains("Build the landing page"));
    capture("tasks", &text);
    x.send("7");
    let text = x.wait_for("the Budget tab", |s| {
        s.contains("spent of this project's $50.00 budget")
    });
    capture("budget", &text);
    x.send("8");
    let text = x.wait_for("the Log tab", |s| s.contains("Charged"));
    capture("log", &text);
    x.send("0");
    let text = x.wait_for("the Settings tab", |s| s.contains("Ask me about"));
    capture("settings", &text);

    // An agent three levels down: Tech, then Backend, then Migrations, whose output is shown.
    x.send("3");
    x.wait_for("the Team tab", |s| s.contains("Enter opens an agent."));
    x.send("\r");
    let text = x.wait_for("the CEO's page", |s| {
        s.contains("c agents") && s.contains("Marketing")
    });
    capture("agent-ceo", &text);
    open_row(&mut x, "Tech");
    x.wait_for("Tech's page", |s| s.contains("CEO > Tech"));
    open_row(&mut x, "Backend");
    x.wait_for("Backend's page", |s| s.contains("CEO > Tech > Backend"));
    open_row(&mut x, "Migrations");
    let text = x.wait_for("Migrations' output", |s| {
        s.contains("CEO > Tech > Backend > Migrations") && s.contains("── turn ended ──")
    });
    assert!(text.contains("▸ write migrations/0001.sql"), "{text}");
    capture("agent-output", &text);
    x.send("w");
    let text = x.wait_for("its waits", |s| s.contains("WAITING FOR"));
    capture("agent-waits", &text);

    // The list of projects, then out.
    x.send("\x1b\x1b\x1b");
    x.send("p");
    let text = x.wait_for("the list", |s| s.contains("x100 · projects"));
    assert!(text.contains("Fernleaf"), "{text}");
    capture("projects", &text);
    x.send("\r");
    x.wait_for("the project again", |s| s.contains("1 Chat"));
    x.send("\x1b[Z"); // back-tab to Settings
    let status = x.quit();
    assert!(status.success(), "{status:?}");

    // The next run opens on the same project, and on the tab left open.
    let mut again = Pty::spawn(&run);
    let text = again.wait_for("the same project", |s| s.contains("x100 · Fernleaf"));
    assert!(
        !text.contains("Tell the CEO what you want to build."),
        "{text}"
    );
    assert!(
        text.contains("Ask me about"),
        "it opens on Settings: {text}"
    );
    let status = again.quit();
    assert!(status.success(), "{status:?}");
}
