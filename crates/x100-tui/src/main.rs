//! `x100 [--socket PATH] [--daemon PATH] [COMMAND]`: the terminal interface. It finds the daemon
//! the way the desktop app does, starting `x100d` beside it if nothing answers.

use std::process::ExitCode;

use x100_client::{Client, ClientError, Options};
use x100_tui::args::{self, Command};
use x100_tui::cli;
use x100_tui::remember::Remember;

fn main() -> ExitCode {
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("x100: {e}");
            eprint!("{}", args::USAGE);
            return ExitCode::from(cli::USAGE_ERROR);
        }
    };
    if args.command == Some(Command::Help) {
        cli::help();
        return ExitCode::SUCCESS;
    }
    let options = match Options::from_flags("x100", args.socket, args.daemon) {
        Ok(mut o) => {
            if let Some(secs) = args.connect_timeout {
                o.startup_timeout = std::time::Duration::from_secs(secs);
            }
            o
        }
        Err(e) => {
            eprintln!("x100: {e}");
            return ExitCode::from(cli::USAGE_ERROR);
        }
    };
    let remember = Remember::beside(&options.socket);
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("x100: could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(async {
        let client = match Client::connect(options).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("x100: could not reach the daemon: {e}");
                return match e {
                    ClientError::NoDaemon(_) => cli::NO_DAEMON,
                    _ => cli::FAILED,
                };
            }
        };
        match args.command {
            Some(command) => cli::exit_code(cli::run(client, command, &remember).await),
            None => match x100_tui::run::run(client, remember).await {
                Ok(()) => cli::DONE,
                Err(e) => {
                    eprintln!("x100: {e}");
                    cli::FAILED
                }
            },
        }
    });
    // The input thread may still be blocked reading the terminal.
    runtime.shutdown_background();
    ExitCode::from(code)
}
