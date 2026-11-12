//! `x100d [--data-dir DIR] [--scripts-dir DIR] [--fake-harness PROGRAM]`

use std::process::ExitCode;

use x100d::{Config, Daemon};

fn main() -> ExitCode {
    let config = match Config::from_args(std::env::args_os().skip(1), |k| std::env::var_os(k)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("x100d: {e}");
            return ExitCode::from(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        let daemon = match Daemon::start(config).await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("x100d: {e}");
                return ExitCode::FAILURE;
            }
        };
        let stop = daemon.stop_handle();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let (mut term, mut int) = (
                signal(SignalKind::terminate()).expect("a SIGTERM handler"),
                signal(SignalKind::interrupt()).expect("a SIGINT handler"),
            );
            tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            stop.now();
        });
        daemon.run().await;
        ExitCode::SUCCESS
    })
}
