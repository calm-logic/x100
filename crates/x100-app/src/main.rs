//! `x100-app`: the desktop app. It connects to the daemon's socket, starting `x100d` if nothing
//! answers, and opens the window.
//!
//! Usage: `x100-app [--socket PATH] [--daemon PATH]`. Without `--socket` the daemon's default
//! socket is used, and an `x100d` beside this program is started if nothing answers there. With
//! `--socket`, a daemon is started only if `--daemon` names one, with the socket's directory as
//! its data directory.

use std::path::PathBuf;
use std::process::ExitCode;

use gpui::App;
use x100_client::{Client, Launch, Options};

struct Args {
    socket: Option<PathBuf>,
    daemon: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let mut args = Args {
        socket: None,
        daemon: None,
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{} needs a path", arg.to_string_lossy()))
        };
        match arg.to_str() {
            Some("--socket") => args.socket = Some(value()?),
            Some("--daemon") => args.daemon = Some(value()?),
            Some("-h" | "--help") => {
                return Err("usage: x100-app [--socket PATH] [--daemon PATH]".into());
            }
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy())),
        }
    }
    Ok(args)
}

fn options(args: Args) -> Result<Options, String> {
    let launch = match args.socket {
        // A daemon started with no arguments listens on the default socket.
        None => args
            .daemon
            .or_else(|| {
                let exe = std::env::current_exe().ok()?;
                Some(exe.parent()?.join("x100d")).filter(|p| p.exists())
            })
            .map(|program| Launch {
                program,
                args: Vec::new(),
            }),
        Some(ref socket) => args
            .daemon
            .zip(socket.parent())
            .map(|(program, dir)| Launch {
                program,
                args: vec!["--data-dir".into(), dir.into()],
            }),
    };
    let socket = args
        .socket
        .or_else(x100_client::default_socket)
        .ok_or("no socket: pass --socket, or set X100_DATA_DIR or HOME")?;
    let mut options = Options::new(socket, "x100-app");
    options.launch = launch;
    Ok(options)
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let options = match args()
        .and_then(options)
        .and_then(|o| x100_app::platform::display_named().map(|()| o))
    {
        Ok(options) => options,
        Err(e) => {
            eprintln!("x100-app: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The client's connection runs on Tokio. GPUI's tasks only await its channels, which need no
    // runtime of their own.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("x100-app: could not start: {e}");
            return ExitCode::FAILURE;
        }
    };
    let client = match runtime.block_on(Client::connect(options)) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("x100-app: could not reach the daemon: {e}");
            return ExitCode::FAILURE;
        }
    };
    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let failure = failed.clone();
    let ran = x100_app::platform::run(move |cx: &mut App| {
        x100_app::init(cx);
        cx.set_reduce_motion(x100_app::platform::reduces_motion());
        if let Err(e) = x100_app::open_window(cx, client) {
            eprintln!("x100-app: could not open a window: {e:#}");
            failure.set(true);
            cx.quit();
            return;
        }
        x100_app::platform::started();
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
    runtime.shutdown_background();
    if let Err(e) = ran {
        eprintln!("x100-app: {e}");
        return ExitCode::FAILURE;
    }
    if failed.get() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
