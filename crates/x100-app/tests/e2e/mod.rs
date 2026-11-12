//! What the end-to-end run on real logins keeps and checks after the work is done: its account
//! in numbers, the records it keeps for looking at, the page in a browser, and that nothing is
//! left running.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use x100_app::term::{Size, screen_text};
use x100_client::Client;
use x100_core::driver::DriverEvent;
use x100_core::event::ProjectEvent;
use x100_core::fold::ProjectState;
use x100_core::id::{AgentId, DriverId, ProjectId};
use x100_core::message::Route;
use x100_core::terminal::ChunkKind;
use x100_proto::{LogId, LogPage, Page, ReadLog, ReadTerminal, ReadTranscript};

const CHROMIUM: &str = "/snap/bin/chromium";

/// The run in numbers, as the daemon's log has it.
pub struct Account {
    turns: usize,
    drivers: Vec<String>,
    agents: Vec<String>,
    tokens: String,
    cost: f64,
    took: Duration,
    replies: Vec<String>,
}

impl Account {
    pub fn read(
        rt: &tokio::runtime::Runtime,
        client: &Client,
        project: ProjectId,
        p: &ProjectState,
        took: Duration,
    ) -> Account {
        let events = read_log(rt, client, project);
        let turns = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ProjectEvent::Driver {
                        event: DriverEvent::TurnStarted { .. },
                        ..
                    }
                )
            })
            .count();
        let daemon = client.state().daemon;
        let role = |a: AgentId| p.agent(a).map_or("?".into(), |r| r.role.clone());
        let drivers = p
            .drivers()
            .map(|d| {
                format!(
                    "{} for {}: {} {} at {:?}, {} turns, {:?}",
                    d.driver.id,
                    role(d.driver.agent),
                    x100_app::words::harness(&daemon, d.driver.harness),
                    d.driver.model.0,
                    d.driver.effort,
                    d.turns,
                    d.state
                )
            })
            .collect();
        let ceo = p.project().map(|pr| pr.ceo);
        let agents = p
            .agents()
            .filter(|a| Some(a.id) != ceo)
            .map(|a| a.role.clone())
            .collect();
        let budget = p.budget();
        let tokens = budget.as_ref().map_or(String::new(), |b| {
            let u = &b.usage;
            format!(
                "{} input, {} output, {} read from cache, {} written to cache",
                u.input,
                u.output,
                u.cache_read,
                u.cache_write_short + u.cache_write_long
            )
        });
        let cost = budget.map_or(0., |b| b.project.spent.micros() as f64 / 1e6);
        let replies = p
            .messages()
            .filter(|m| m.message.route == Route::CeoToBuilder)
            .map(|m| m.message.body.clone())
            .collect();
        Account {
            turns,
            drivers,
            agents,
            tokens,
            cost,
            took,
            replies,
        }
    }
}

impl std::fmt::Display for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "THE RUN")?;
        writeln!(f, "Took {:.0?}, {} model turns.", self.took, self.turns)?;
        writeln!(f, "Drivers:")?;
        for d in &self.drivers {
            writeln!(f, "  {d}")?;
        }
        if self.agents.is_empty() {
            writeln!(f, "Agents formed: none, the CEO worked alone.")?;
        } else {
            writeln!(f, "Agents formed: {}.", self.agents.join(", "))?;
        }
        writeln!(f, "Tokens: {}.", self.tokens)?;
        writeln!(f, "Listed cost: ${:.4}.", self.cost)?;
        writeln!(f, "The CEO's replies in the chat:")?;
        for r in &self.replies {
            writeln!(f, "  > {}", r.replace('\n', "\n    "))?;
        }
        Ok(())
    }
}

pub fn read_log(
    rt: &tokio::runtime::Runtime,
    client: &Client,
    project: ProjectId,
) -> Vec<ProjectEvent> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let page = rt
            .block_on(client.request(ReadLog {
                log: LogId::Project(project),
                page: Page::OldestFirst { after, limit: 500 },
            }))
            .expect("the log");
        let LogPage::Project { events } = page else {
            panic!("a project's log");
        };
        let Some(last) = events.last() else {
            return out;
        };
        after = Some(last.seq);
        out.extend(events.into_iter().map(|e| e.event));
    }
}

pub fn read_terminal(
    rt: &tokio::runtime::Runtime,
    client: &Client,
    project: ProjectId,
    driver: DriverId,
) -> Vec<ChunkKind> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let page = rt
            .block_on(client.request(ReadTerminal {
                project,
                driver,
                after,
                limit: 1000,
            }))
            .expect("the terminal");
        let Some(last) = page.chunks.last() else {
            return out;
        };
        after = Some(last.position);
        out.extend(page.chunks.into_iter().map(|c| c.chunk.what));
    }
}

/// Each agent's transcript as JSON lines, and each driver's terminal as its last screen, in `dir`.
pub fn keep_records(
    rt: &tokio::runtime::Runtime,
    client: &Client,
    project: ProjectId,
    p: &ProjectState,
    dir: &Path,
) {
    for a in p.agents() {
        let mut lines = String::new();
        let mut after = None;
        loop {
            let page = rt
                .block_on(client.request(ReadTranscript {
                    project,
                    agent: a.id,
                    after,
                    limit: 500,
                }))
                .expect("a transcript");
            let Some(last) = page.entries.last() else {
                break;
            };
            after = Some(last.position);
            for e in &page.entries {
                lines.push_str(&serde_json::to_string(&e.entry).unwrap());
                lines.push('\n');
            }
        }
        let _ = std::fs::write(dir.join(format!("transcript-{}.jsonl", a.role)), lines);
    }
    for d in p.drivers() {
        let screen = last_screen(&read_terminal(rt, client, project, d.driver.id));
        let _ = std::fs::write(dir.join(format!("terminal-{}.txt", d.driver.id)), screen);
    }
}

fn last_screen(chunks: &[ChunkKind]) -> String {
    let size = |cols: u16, rows: u16| Size {
        cols: cols.into(),
        rows: rows.into(),
    };
    let Some(ChunkKind::Resize { cols, rows }) = chunks.first() else {
        return String::new();
    };
    let mut term = Term::new(Config::default(), &size(*cols, *rows), VoidListener);
    let mut parser: Processor = Processor::new();
    for chunk in chunks {
        match chunk {
            ChunkKind::Resize { cols, rows } => term.resize(size(*cols, *rows)),
            ChunkKind::Output { bytes } => parser.advance(&mut term, &bytes.0),
        }
    }
    screen_text(&term).join("\n")
}

pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Opens the page in headless Chromium twice on one profile: once adding a todo through the page's
/// own input, once fresh, as a reload. Says what each saw, or why it could not look.
/// Why the page was not seen to work in a browser.
pub enum Browser {
    /// There is no browser to look with.
    Missing(String),
    Failed(String),
}

pub fn browser_check(html: &str, scratch: &Path) -> Result<String, Browser> {
    if !Path::new(CHROMIUM).exists() {
        return Err(Browser::Missing(format!("{CHROMIUM} is not installed")));
    }
    // Under the home directory, which the snap can read; its own /tmp is private.
    let dir = scratch.join("browser");
    std::fs::create_dir_all(&dir).map_err(|e| Browser::Missing(e.to_string()))?;
    let page = dir.join("index.html");
    let added = run_page(&page, &dir, html, ADD).map_err(Browser::Missing)?;
    let reloaded = run_page(&page, &dir, html, CHECK).map_err(Browser::Missing)?;
    let said = format!("adding a todo gave \"{added}\", a reload gave \"{reloaded}\"");
    if added == "x100:added" && reloaded == "x100:kept" {
        Ok(said)
    } else {
        Err(Browser::Failed(said))
    }
}

/// Whether the page shows `text`, outside its scripts and styles.
const SHOWN: &str = r#"
function shown(text) {
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    if (!/^(SCRIPT|STYLE)$/.test(n.parentElement.tagName) && n.nodeValue.includes(text)) return true;
  }
  return false;
}
"#;

/// Types into the page's text input and adds it by the page's own means: its form, a button that
/// says Add, the first button after the input, or Enter. The text is put together at run time, so
/// the script's own source never matches it.
const ADD: &str = r#"
window.addEventListener('load', () => setTimeout(() => {
  const text = ['x100', 'todo', 'check'].join(' ');
  const input = [...document.querySelectorAll('input, textarea')].find(i => !['checkbox', 'radio', 'button', 'submit', 'hidden'].includes(i.type));
  if (!input) { document.title = 'x100:no-input'; return; }
  input.focus();
  input.value = text;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  const has = () => shown(text);
  const buttons = [...document.querySelectorAll('button, input[type=submit], input[type=button]')];
  const label = b => b.textContent + ' ' + (b.value || '') + ' ' + (b.getAttribute('aria-label') || '') + ' ' + (b.title || '');
  const steps = [
    () => { if (input.form) input.form.requestSubmit(); },
    () => { const b = buttons.find(b => /add/i.test(label(b))); if (b) b.click(); },
    () => { const b = buttons.find(b => input.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING); if (b) b.click(); },
    () => { for (const type of ['keydown', 'keypress', 'keyup']) input.dispatchEvent(new KeyboardEvent(type, { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true })); },
  ];
  for (const step of steps) { if (has()) break; step(); }
  setTimeout(() => { document.title = has() ? 'x100:added' : 'x100:not-added'; }, 300);
}, 300));
"#;

const CHECK: &str = r#"
window.addEventListener('load', () => setTimeout(() => {
  const text = ['x100', 'todo', 'check'].join(' ');
  document.title = shown(text) ? 'x100:kept' : 'x100:lost';
}, 400));
"#;

fn run_page(page: &Path, dir: &Path, html: &str, script: &str) -> Result<String, String> {
    std::fs::write(page, format!("{html}\n<script>{SHOWN}{script}</script>\n"))
        .map_err(|e| e.to_string())?;
    // Chromium's sockets go under TMPDIR, whose path here is otherwise too long for one.
    let tmp = Path::new("/tmp/c");
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let out = Command::new(CHROMIUM)
        .env("TMPDIR", tmp)
        .args([
            "--headless",
            "--no-sandbox",
            "--disable-gpu",
            "--virtual-time-budget=5000",
        ])
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg("--dump-dom")
        .arg(format!("file://{}", page.display()))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let dom = String::from_utf8_lossy(&out.stdout);
    let title = dom
        .split("<title>")
        .nth(1)
        .and_then(|t| t.split("</title>").next())
        .unwrap_or("no title");
    Ok(title.to_owned())
}

/// Fails if any process still names one of `dirs` in its command line or environment, which every
/// driver process does through its home or its working directory. Looks at every process, not only
/// a group, since the programs start helpers in sessions of their own.
pub fn nothing_left(dirs: &[&Path]) {
    let me = std::process::id();
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let left: Vec<String> = std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| *pid != me)
            .filter_map(|pid| {
                let read = |f: &str| std::fs::read(format!("/proc/{pid}/{f}")).unwrap_or_default();
                let all = [read("cmdline"), read("environ")].concat();
                let text = String::from_utf8_lossy(&all);
                dirs.iter()
                    .any(|d| text.contains(&*d.to_string_lossy()))
                    .then(|| {
                        format!(
                            "{pid}: {}",
                            String::from_utf8_lossy(&read("cmdline")).replace('\0', " ")
                        )
                    })
            })
            .collect();
        if left.is_empty() {
            return;
        }
        assert!(Instant::now() < until, "left running: {left:#?}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn installed(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// A subscription login as Claude Code writes it, with dummy tokens.
pub fn dummy_login(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 8 * 3600 * 1000;
    let credentials = serde_json::json!({ "claudeAiOauth": {
        "accessToken": "sk-ant-oat01-x100-dummy-access-token",
        "refreshToken": "sk-ant-ort01-x100-dummy-refresh",
        "expiresAt": expires,
        "scopes": ["user:inference", "user:profile", "user:sessions:claude_code"],
        "subscriptionType": "max",
    }});
    std::fs::write(dir.join(".credentials.json"), credentials.to_string()).unwrap();
}

/// What the fake model writes: a todo page that does what the brief asks.
pub const TODO_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Todo</title>
<style>body { font-family: sans-serif; max-width: 30em; margin: 2em auto; } .done span { text-decoration: line-through; }</style>
</head><body>
<form id="add"><input id="text" placeholder="What needs doing?"><button>Add</button></form>
<ul id="list"></ul>
<script>
const key = 'todos';
let todos = JSON.parse(localStorage.getItem(key) || '[]');
const list = document.getElementById('list');
function save() { localStorage.setItem(key, JSON.stringify(todos)); draw(); }
function draw() {
  list.innerHTML = '';
  todos.forEach((t, i) => {
    const li = document.createElement('li');
    li.className = t.done ? 'done' : '';
    const box = document.createElement('input');
    box.type = 'checkbox';
    box.checked = t.done;
    box.onchange = () => { todos[i].done = box.checked; save(); };
    const span = document.createElement('span');
    span.textContent = t.text;
    const del = document.createElement('button');
    del.textContent = 'Delete';
    del.onclick = () => { todos.splice(i, 1); save(); };
    li.append(box, span, del);
    list.append(li);
  });
}
document.getElementById('add').onsubmit = e => {
  e.preventDefault();
  const input = document.getElementById('text');
  if (input.value.trim()) { todos.push({ text: input.value.trim(), done: false }); input.value = ''; save(); }
};
draw();
</script>
</body></html>
"#;
