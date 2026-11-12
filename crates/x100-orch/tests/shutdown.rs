//! When shutdown returns, nothing of the orchestrator's runs and no driver is on record, however
//! late a driver's own task is in finishing.

mod common;

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use common::*;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::stat::Mode;

const SCRIPT: &str = r#"
[harness]
name = "fake-shutdown"

[[model]]
model = "fake-long"
context_window = 200000
cache_lifetime_secs = 604800
efforts = ["low", "medium", "high", "max"]
price = { input = 1.0, output = 5.0 }

[[turn]]
repeat = true
steps = [
  { call = { schedule = { when = { once = { at = 1794423600000 } }, does = { wake = { agent = "{{pack:Your id}}", note = "Later." } } } }, save = "later" },
  { call = { wait = { on = { job = "{{later.job}}" }, task = "{{input:Current task}}", progress = { note = "Waiting.", done = 0 } } } },
]
"#;

/// The process list is written to `drivers.part` and renamed into place. A full pipe there holds
/// the next write, which is the one a stopped driver's task makes as it finishes, until the test
/// reads from the pipe. Returns the reading end and how many bytes fill it.
fn hold_the_next_write(part: &std::path::Path) -> (File, usize) {
    nix::unistd::mkfifo(part, Mode::S_IRWXU).expect("a pipe");
    let nonblocking = OFlag::O_NONBLOCK.bits();
    let reader = OpenOptions::new()
        .read(true)
        .custom_flags(nonblocking)
        .open(part)
        .expect("the pipe opens for reading");
    let mut writer = OpenOptions::new()
        .write(true)
        .custom_flags(nonblocking)
        .open(part)
        .expect("the pipe opens for writing");
    let mut filled = 0;
    loop {
        match writer.write(&[b' '; 4096]) {
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => panic!("filling the pipe: {e}"),
        }
    }
    // Reads block from here on, and end once no writer has the pipe open.
    fcntl(reader.as_raw_fd(), FcntlArg::F_SETFL(OFlag::empty())).expect("a blocking reader");
    (reader, filled)
}

fn read_all(mut reader: File) -> (File, Vec<u8>) {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).expect("the pipe reads");
    (reader, bytes)
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_returns_only_once_every_driver_task_has_finished() {
    let w = World::new(Setup::new(SCRIPT)).await;
    w.send("Get started.").await;
    w.settle().await;
    let driver = w.state().acting_driver(w.ceo).expect("a warm driver");
    let (reader, filled) = hold_the_next_write(&w.dir.path().join("drivers.part"));

    let orch = w.orch.clone();
    let mut shutdown = tokio::spawn(async move { orch.shutdown().await });
    w.until("the driver's end is recorded", |s| {
        !s.driver(driver).unwrap().state.is_live()
    })
    .await;
    // The driver has ended and its task is on its last lines, held at the write. Shutdown must
    // still be waiting for it; the old shutdown returned here, leaving it on record.
    let early = tokio::time::timeout(Duration::from_millis(500), &mut shutdown).await;
    assert!(
        early.is_err(),
        "shutdown returned while a driver's task was still writing its process list"
    );

    let (reader, mut written) = tokio::task::spawn_blocking(move || read_all(reader))
        .await
        .unwrap();
    shutdown
        .await
        .expect("shutdown finishes once the write goes through");
    // Whatever the write left in the pipe after the first read ended.
    let (_, rest) = tokio::task::spawn_blocking(move || read_all(reader))
        .await
        .unwrap();
    written.extend(rest);
    assert_eq!(
        String::from_utf8_lossy(&written[filled..]),
        "{}",
        "the last process list names no driver"
    );
    w.assert_log_folds();
}
