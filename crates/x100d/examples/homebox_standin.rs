//! The tests' Homebox stand-in as a program, for the window's end-to-end script to run its dry
//! run against: Homebox's API on one loopback port, the deployed sites on another, and stand-in
//! `docker` and `gh` programs written beside it that find it and are logged in. Nothing it does
//! leaves the machine: GitHub is the bare repositories under `--remotes`.
//!
//! Usage: homebox_standin --remotes DIR --bin DIR --key KEY
//!
//! Prints `api PORT site PORT` once both listen, then serves until it is stopped. A site is asked
//! for by its public address in the Host header, `<project>.box.x100.test`, over plain HTTP.

#[path = "../tests/standin/homebox.rs"]
mod homebox;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use homebox::{ACCOUNT, HomeboxStandIn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn program(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("the program is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the program is made runnable");
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (mut remotes, mut bin, mut key) = (None, None, None);
    while let Some(a) = args.next() {
        let value = args.next();
        match a.as_str() {
            "--remotes" => remotes = value.map(PathBuf::from),
            "--bin" => bin = value.map(PathBuf::from),
            "--key" => key = value,
            other => panic!("unknown argument {other}"),
        }
    }
    let (remotes, bin, key) = (
        remotes.expect("--remotes DIR"),
        bin.expect("--bin DIR"),
        key.expect("--key KEY"),
    );
    std::fs::create_dir_all(remotes.join(ACCOUNT)).expect("the remotes directory is made");
    std::fs::create_dir_all(&bin).expect("the programs' directory is made");

    let hb = HomeboxStandIn::start(&remotes).await;
    hb.key(&key, "edit");
    program(
        &bin,
        "docker",
        &homebox::docker_program(true, hb.port, &bin, &key),
    );
    program(&bin, "gh", &homebox::gh_program(true, &remotes, &bin));

    let sites = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let site_port = sites.local_addr().expect("its address").port();
    println!("api {} site {site_port}", hb.port);
    loop {
        let Ok((mut tcp, _)) = sites.accept().await else {
            continue;
        };
        let hb = hb.clone();
        tokio::spawn(async move {
            let mut head = Vec::new();
            let mut chunk = [0u8; 4096];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match tcp.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => head.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&head);
            let host = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("host").then(|| v.trim().to_owned())
                })
                .unwrap_or_default();
            let (status, body) = match hb.site(&host) {
                Some(page) => ("200 OK", page),
                None => ("404 Not Found", format!("nothing is deployed at {host}\n")),
            };
            let answer = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tcp.write_all(answer.as_bytes()).await;
            let _ = tcp.shutdown().await;
        });
    }
}
