//! The single commands: no full screen, one record per line, for scripts and pipelines.

use std::io::Write;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use x100_client::{Client, ClientError, words};
use x100_core::action::Decision;
use x100_core::fold::ChatItem;
use x100_core::id::ProjectId;
use x100_core::message::Route;

use crate::args::{Command, Follow, USAGE};
use crate::logfeed::{self, CATCH_UP};
use crate::remember::Remember;

pub const DONE: u8 = 0;
pub const FAILED: u8 = 1;
pub const USAGE_ERROR: u8 = 2;
pub const NO_DAEMON: u8 = 3;
pub const TIMED_OUT: u8 = 4;

/// What ended a command early.
#[derive(Debug)]
pub enum Stop {
    Usage(String),
    Failed(String),
    Timeout,
    /// The reader of our output went away, as `head` does. Not an error.
    Closed,
}

impl From<ClientError> for Stop {
    fn from(e: ClientError) -> Stop {
        Stop::Failed(e.to_string())
    }
}

impl From<std::io::Error> for Stop {
    fn from(e: std::io::Error) -> Stop {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            Stop::Closed
        } else {
            Stop::Failed(e.to_string())
        }
    }
}

pub fn exit_code(result: Result<(), Stop>) -> u8 {
    match result {
        Ok(()) | Err(Stop::Closed) => DONE,
        Err(Stop::Usage(m)) => {
            eprintln!("x100: {m}");
            USAGE_ERROR
        }
        Err(Stop::Failed(m)) => {
            eprintln!("x100: {m}");
            FAILED
        }
        Err(Stop::Timeout) => {
            eprintln!("x100: timed out");
            TIMED_OUT
        }
    }
}

pub fn help() {
    print!("{USAGE}");
}

/// One field of a record: a tab or a line break in it would split the record.
fn field(s: &str) -> String {
    s.replace(['\t', '\r'], " ").replace('\n', "\\n")
}

fn out(line: &str) -> Result<(), Stop> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")?;
    stdout.flush()?;
    Ok(())
}

/// The project a command is about.
fn project_for(
    client: &Client,
    given: Option<ProjectId>,
    remember: &Remember,
) -> Result<ProjectId, Stop> {
    let env = std::env::var("X100_PROJECT")
        .ok()
        .and_then(|v| v.parse::<ProjectId>().ok());
    let held: Vec<ProjectId> = client.with_state(|s| s.projects.keys().copied().collect());
    if let Some(p) = given.or(env).or(remember.read().project) {
        return if held.contains(&p) {
            Ok(p)
        } else {
            Err(Stop::Failed(format!("no project {p}")))
        };
    }
    match held.as_slice() {
        [only] => Ok(*only),
        [] => Err(Stop::Failed(
            "there is no project yet; make one with `x100 new`".into(),
        )),
        _ => Err(Stop::Usage(
            "more than one project: give --project ID".into(),
        )),
    }
}

async fn message(text: String) -> Result<String, Stop> {
    if text != "-" {
        return Ok(text);
    }
    let mut s = String::new();
    tokio::io::stdin()
        .read_to_string(&mut s)
        .await
        .map_err(|e| Stop::Failed(format!("reading stdin: {e}")))?;
    Ok(s.trim_end().to_owned())
}

pub async fn run(client: Client, command: Command, remember: &Remember) -> Result<(), Stop> {
    match command {
        Command::Help => {
            help();
            Ok(())
        }
        Command::Projects => {
            let rows: Vec<String> = client.with_state(|s| {
                s.projects
                    .iter()
                    .map(|(id, p)| {
                        format!(
                            "{id}\t{}\t{}\t{}",
                            field(&words::project_name(Some(p))),
                            p.agent_count(),
                            if p.is_live() { "live" } else { "idle" }
                        )
                    })
                    .collect()
            });
            rows.iter().try_for_each(|r| out(r))
        }
        Command::New { dir, message: text } => {
            let text = match text {
                Some(t) => Some(message(t).await?),
                None => None,
            };
            let id = client.create_project(dir).await?;
            if let Some(t) = text.filter(|t| !t.is_empty()) {
                client.send_chat(id, t, vec![], None).await?;
            }
            out(&id.to_string())
        }
        Command::Send {
            project,
            message: text,
        } => {
            let id = project_for(&client, project, remember)?;
            let body = message(text).await?;
            if body.is_empty() {
                return Err(Stop::Usage("the message is empty".into()));
            }
            // A message answers the question the CEO is waiting on, as in the full screen.
            let reply = client.project(id).and_then(|p| {
                let mut open: Vec<_> = p
                    .open_questions()
                    .filter(|m| m.message.route == Route::CeoToBuilder)
                    .collect();
                open.sort_by_key(|m| m.at);
                open.first().map(|m| m.message.id)
            });
            let sent = client.send_chat(id, body, vec![], reply).await?;
            out(&sent.to_string())
        }
        Command::Actions { project } => {
            let id = project_for(&client, project, remember)?;
            let rows = client.with_state(|s| {
                let (Some(p), d) = (s.projects.get(&id), &s.daemon) else {
                    return Vec::new();
                };
                let files = Default::default();
                let names = x100_client::wording::Names {
                    project: p,
                    daemon: d,
                    files: &files,
                };
                let mut held: Vec<_> = p.held_actions().collect();
                held.sort_by_key(|a| (a.proposed, a.action.id));
                held.into_iter()
                    .map(|a| {
                        format!(
                            "{}\t{}\t{}\t{}",
                            a.action.id,
                            field(
                                &p.agent(a.action.agent)
                                    .map_or_else(String::new, |x| x.role.clone())
                            ),
                            a.classification
                                .as_ref()
                                .map_or("", |c| words::risk(c.risk)),
                            field(&x100_client::wording::request(&names, &a.action.request)),
                        )
                    })
                    .collect::<Vec<_>>()
            });
            rows.iter().try_for_each(|r| out(r))
        }
        Command::Decide {
            project,
            action,
            approve,
        } => {
            let id = project_for(&client, project, remember)?;
            let decision = if approve {
                Decision::Approved
            } else {
                Decision::Denied
            };
            client.decide_action(id, action, decision).await?;
            out(&format!(
                "{action}\t{}",
                if approve { "approved" } else { "denied" }
            ))
        }
        Command::Log {
            project,
            last,
            follow,
        } => {
            let id = project_for(&client, project, remember)?;
            log(&client, id, last, follow).await
        }
        Command::Chat { project, follow } => {
            let id = project_for(&client, project, remember)?;
            chat(&client, id, follow).await
        }
    }
}

/// Prints each line, and says whether `until` was met by it.
struct Printer<'a> {
    until: Option<&'a str>,
}

impl Printer<'_> {
    fn line(&self, text: &str) -> Result<bool, Stop> {
        out(text)?;
        Ok(self.until.is_some_and(|u| text.contains(u)))
    }
}

/// Waits for the client's state to change, up to the time left.
async fn changed(
    changes: &mut tokio::sync::broadcast::Receiver<x100_client::Change>,
    deadline: Option<Instant>,
) -> Result<(), Stop> {
    let wait = Duration::from_secs(1);
    let wait = deadline.map_or(wait, |d| {
        d.saturating_duration_since(Instant::now()).min(wait)
    });
    let _ = tokio::time::timeout(wait, changes.recv()).await;
    if deadline.is_some_and(|d| Instant::now() >= d) {
        return Err(Stop::Timeout);
    }
    Ok(())
}

async fn log(
    client: &Client,
    id: ProjectId,
    last: Option<usize>,
    follow: Follow,
) -> Result<(), Stop> {
    let deadline = follow
        .timeout_secs
        .map(|s| Instant::now() + Duration::from_secs(s));
    let printer = Printer {
        until: follow.until.as_deref(),
    };
    let mut files = Default::default();
    let mut changes = client.changes();
    let format = |events: &[_], files: &mut _| -> Vec<(x100_core::event::Seq, String)> {
        client.with_state(|s| {
            let Some(p) = s.projects.get(&id) else {
                return Vec::new();
            };
            logfeed::rows(events, p, &s.daemon, files)
                .into_iter()
                .map(|r| {
                    (
                        r.seq,
                        format!(
                            "{}\t{}\t{}\t{}",
                            r.seq.0,
                            r.at,
                            field(&r.who),
                            field(&r.text)
                        ),
                    )
                })
                .collect()
        })
    };
    // History, oldest first, then whatever follows the newest event printed.
    let mut newest = None;
    let mut before = None;
    let mut pages = Vec::new();
    let mut want = last.unwrap_or(usize::MAX);
    while want > 0 {
        let page = u32::try_from(want.min(logfeed::PAGE as usize)).unwrap_or(logfeed::PAGE);
        let mut events = client.read_events_before(id, before, page).await?;
        if events.is_empty() {
            break;
        }
        before = events.last().map(|e| e.seq);
        newest = newest.or(events.first().map(|e| e.seq));
        events.reverse();
        want = want.saturating_sub(events.len());
        let done = events.first().is_some_and(|e| e.seq.0 <= 1);
        pages.push(events);
        if done {
            break;
        }
    }
    for events in pages.iter().rev() {
        for (_, line) in format(events, &mut files) {
            if printer.line(&line)? {
                return Ok(());
            }
        }
    }
    if !follow.follow {
        return if follow.until.is_some() {
            Err(Stop::Timeout)
        } else {
            Ok(())
        };
    }
    loop {
        let held = client.project(id).and_then(|p| p.seq());
        if held > newest {
            let events = client.read_events_after(id, newest, CATCH_UP).await?;
            newest = events.last().map(|e| e.seq).or(newest);
            for (_, line) in format(&events, &mut files) {
                if printer.line(&line)? {
                    return Ok(());
                }
            }
            continue;
        }
        changed(&mut changes, deadline).await?;
    }
}

async fn chat(client: &Client, id: ProjectId, follow: Follow) -> Result<(), Stop> {
    let deadline = follow
        .timeout_secs
        .map(|s| Instant::now() + Duration::from_secs(s));
    let printer = Printer {
        until: follow.until.as_deref(),
    };
    let mut changes = client.changes();
    let mut printed = None;
    loop {
        let lines: Vec<(x100_core::event::Seq, String)> = client
            .project(id)
            .map(|p| {
                p.chat()
                    .iter()
                    .filter(|e| Some(e.seq) > printed)
                    .filter_map(|e| {
                        let text = match &e.item {
                            ChatItem::Message(m) => {
                                let m = p.message(*m)?;
                                let who = match m.message.route {
                                    Route::BuilderToCeo => "you",
                                    Route::DaemonToBuilder => "x100",
                                    _ => "ceo",
                                };
                                format!("{}\t{who}\t{}", e.seq.0, field(&m.message.body))
                            }
                            ChatItem::Report { task, report } => format!(
                                "{}\tceo\tReport on {}: {}",
                                e.seq.0,
                                field(
                                    &p.task(*task)
                                        .map_or_else(|| "a task".into(), |t| t.task.title.clone())
                                ),
                                field(&report.summary)
                            ),
                        };
                        Some((e.seq, text))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (seq, line) in lines {
            printed = Some(seq);
            if printer.line(&line)? {
                return Ok(());
            }
        }
        if !follow.follow {
            return if follow.until.is_some() {
                Err(Stop::Timeout)
            } else {
                Ok(())
            };
        }
        changed(&mut changes, deadline).await?;
    }
}
