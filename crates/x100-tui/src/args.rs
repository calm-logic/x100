//! The command line: where the daemon is, and which command to run. No command opens the full
//! screen.

use std::path::PathBuf;

use x100_core::id::{ActionId, ProjectId};

pub const USAGE: &str = "\
usage: x100 [--socket PATH] [--daemon PATH] [--connect-timeout SECS] [COMMAND]

With no command, x100 opens the full screen.

commands:
  projects                         list projects: id, name, agents, live or idle
  new [--dir DIR] [MESSAGE|-]      make a project, send MESSAGE to its CEO if given, print its id
  send [--project ID] MESSAGE|-    send a message to the project's CEO, print its id
  chat [--project ID] [-f] [--until TEXT] [--timeout SECS]
                                   print the chat: seq, who, text
  log [--project ID] [-n N | --all] [-f] [--until TEXT] [--timeout SECS]
                                   print the log, oldest first: seq, time, who, text
  actions [--project ID]           list the actions waiting for you
  integrations                     what is connected: homebox, address, primary domain; github, account
  approve ID [--project ID]        approve a held action
  deny ID [--project ID]           deny a held action

Output is one record per line, fields separated by a tab. The project defaults to $X100_PROJECT,
then the one last open in the full screen, then the only one.

exit codes: 0 done, 1 refused or failed, 2 wrong usage, 3 no daemon answered, 4 timed out
";

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub socket: Option<PathBuf>,
    pub daemon: Option<PathBuf>,
    /// How long to look for the daemon, starting it if need be.
    pub connect_timeout: Option<u64>,
    pub command: Option<Command>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Projects,
    New {
        dir: Option<String>,
        message: Option<String>,
    },
    Send {
        project: Option<ProjectId>,
        message: String,
    },
    Chat {
        project: Option<ProjectId>,
        follow: Follow,
    },
    Log {
        project: Option<ProjectId>,
        last: Option<usize>,
        follow: Follow,
    },
    Actions {
        project: Option<ProjectId>,
    },
    Integrations,
    Decide {
        project: Option<ProjectId>,
        action: ActionId,
        approve: bool,
    },
}

/// Whether to keep printing as lines arrive, and when to stop.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Follow {
    pub follow: bool,
    pub until: Option<String>,
    pub timeout_secs: Option<u64>,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut it = args.into_iter().peekable();
    let mut socket = None;
    let mut daemon = None;
    let mut connect_timeout = None;
    let mut name = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(it.next().ok_or("--socket needs a path")?)),
            "--daemon" => daemon = Some(PathBuf::from(it.next().ok_or("--daemon needs a path")?)),
            "--connect-timeout" => {
                let v = it.next().ok_or("--connect-timeout needs seconds")?;
                connect_timeout = Some(v.parse().map_err(|_| format!("{v} is not a number"))?);
            }
            "-h" | "--help" | "help" => {
                return Ok(Args {
                    socket,
                    daemon,
                    connect_timeout,
                    command: Some(Command::Help),
                });
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other}"));
            }
            other => {
                name = Some(other.to_owned());
                break;
            }
        }
    }
    let Some(name) = name else {
        return Ok(Args {
            socket,
            daemon,
            connect_timeout,
            command: None,
        });
    };
    let rest: Vec<String> = it.collect();
    let command = command(&name, rest)?;
    Ok(Args {
        socket,
        daemon,
        connect_timeout,
        command: Some(command),
    })
}

struct Flags {
    project: Option<ProjectId>,
    dir: Option<String>,
    last: Option<usize>,
    all: bool,
    follow: Follow,
    positional: Vec<String>,
}

fn flags(rest: Vec<String>) -> Result<Flags, String> {
    let mut f = Flags {
        project: None,
        dir: None,
        last: None,
        all: false,
        follow: Follow::default(),
        positional: Vec::new(),
    };
    let mut it = rest.into_iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| it.next().ok_or_else(|| format!("{a} needs {what}"));
        match a.as_str() {
            "--project" | "-p" => {
                let v = value("a project id")?;
                f.project = Some(v.parse().map_err(|e| format!("{v}: {e}"))?);
            }
            "--dir" => f.dir = Some(value("a folder")?),
            "-n" => {
                let v = value("a number")?;
                f.last = Some(v.parse().map_err(|_| format!("{v} is not a number"))?);
            }
            "--all" => f.all = true,
            "-f" | "--follow" => f.follow.follow = true,
            "--until" => f.follow.until = Some(value("text")?),
            "--timeout" => {
                let v = value("seconds")?;
                f.follow.timeout_secs =
                    Some(v.parse().map_err(|_| format!("{v} is not a number"))?);
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option {other}"));
            }
            _ => f.positional.push(a),
        }
    }
    Ok(f)
}

fn command(name: &str, rest: Vec<String>) -> Result<Command, String> {
    let mut f = flags(rest)?;
    let no_more = |f: &Flags| {
        if f.positional.is_empty() {
            Ok(())
        } else {
            Err("too many arguments".to_owned())
        }
    };
    Ok(match name {
        "projects" => {
            no_more(&f)?;
            Command::Projects
        }
        "new" => {
            if f.positional.len() > 1 {
                return Err("new takes one message; quote it".into());
            }
            Command::New {
                dir: f.dir,
                message: f.positional.pop(),
            }
        }
        "send" => {
            if f.positional.len() != 1 {
                return Err(
                    "send takes one message; quote it, or give - to read it from stdin".into(),
                );
            }
            Command::Send {
                project: f.project,
                message: f.positional.remove(0),
            }
        }
        "chat" => {
            no_more(&f)?;
            Command::Chat {
                project: f.project,
                follow: f.follow,
            }
        }
        "log" => {
            no_more(&f)?;
            let last = if f.all {
                None
            } else {
                Some(f.last.unwrap_or(50))
            };
            Command::Log {
                project: f.project,
                last,
                follow: f.follow,
            }
        }
        "actions" => {
            no_more(&f)?;
            Command::Actions { project: f.project }
        }
        "integrations" => {
            no_more(&f)?;
            Command::Integrations
        }
        "approve" | "deny" => {
            if f.positional.len() != 1 {
                return Err(format!("{name} takes one action id"));
            }
            let id = f.positional.remove(0);
            let action = id.parse().map_err(|e| format!("{id}: {e}"))?;
            Command::Decide {
                project: f.project,
                action,
                approve: name == "approve",
            }
        }
        other => return Err(format!("no command {other}; try x100 help")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn no_command_is_the_full_screen() {
        assert_eq!(p(&[]).unwrap().command, None);
        assert_eq!(p(&["--socket", "/s"]).unwrap().socket, Some("/s".into()));
    }

    #[test]
    fn log_defaults_and_follow() {
        let Command::Log { last, follow, .. } = p(&["log", "-f", "--until", "done"])
            .unwrap()
            .command
            .unwrap()
        else {
            panic!("a log command");
        };
        assert_eq!(last, Some(50));
        assert!(follow.follow);
        assert_eq!(follow.until.as_deref(), Some("done"));
    }

    #[test]
    fn usage_errors_name_the_problem() {
        assert!(p(&["send"]).unwrap_err().contains("one message"));
        assert!(p(&["frob"]).unwrap_err().contains("no command frob"));
        assert!(p(&["--nope"]).unwrap_err().contains("unknown option"));
    }
}
