//! M10's check, written once against [`Backend`] and run on bubblewrap and on Docker.

mod common;

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use common::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use x100_core::id::AgentId;
use x100_sandbox::{Backend, Mount, git_paths};
use x100_store::git::{Git, MergeOutcome};

on_both!(
    contained,
    sandboxes_are_apart,
    git_commits_inside_and_merges_outside,
    next_driver_finds_files_and_server,
    stopping_a_driver_ends_its_background,
    destroy_ends_everything,
    endpoints_reach_the_host,
    lab_has_no_way_out,
    login_link_is_writable,
    real_harnesses_start,
    terminal_reaches_inside,
    host_reaches_a_socket_inside,
);

/// Deleting the home, writing outside the worktree and connecting out all fail or touch only
/// the sandbox. A file of the builder's that is not mapped cannot be read.
async fn contained(backend: &dyn Backend) {
    let base = scratch();
    let home = dir(base.path().join("builder-home"));
    std::fs::write(home.join("secret"), "builder's secret").unwrap();
    let toolchain = dir(home.join("toolchain"));
    std::fs::write(toolchain.join("tool"), "a program").unwrap();
    let work = dir(base.path().join("work"));
    let mut spec = spec("contained", base.path(), &work);
    spec.programs = vec![toolchain.clone()];
    let sandbox = backend.create(spec).await.expect("created");
    std::fs::write(sandbox.home().join("dotfile"), "sandbox's own").unwrap();

    let probe = format!("x100-probe-{}", name("tmp"));
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // The host's own address on its network, found without sending anything.
    let udp = UdpSocket::bind("0.0.0.0:0").unwrap();
    udp.connect("1.1.1.1:80").unwrap();
    let host_ip = udp.local_addr().unwrap().ip();
    let h = home.display();
    let script = format!(
        r#"
        cat {h}/secret && echo READ-SECRET
        rm -rf "$HOME"; ls -A "$HOME"; echo home-listed
        rm -rf {h} 2>/dev/null
        for p in /usr/x100-probe /etc/x100-probe {h}/x100-probe {h}/toolchain/x100-probe; do
          touch $p 2>/dev/null && echo WROTE $p
        done
        touch /tmp/{probe} && echo tmp-ok
        for a in 127.0.0.1/{port} {host_ip}/{port} 1.1.1.1/80; do
          timeout 5 bash -c "echo > /dev/tcp/$a" 2>/dev/null && echo CONNECTED $a
        done
        echo done
        "#
    );
    let out = run(&sandbox, &script).await;
    let said = stdout(&out);
    assert!(said.ends_with("done"), "{}", text(&out));
    assert!(!said.contains("READ-SECRET"), "{}", text(&out));
    assert!(!said.contains("WROTE"), "{}", text(&out));
    assert!(!said.contains("CONNECTED"), "{}", text(&out));
    assert!(said.contains("tmp-ok"), "{}", text(&out));
    // Only the sandbox's own things went.
    assert!(!sandbox.home().join("dotfile").exists());
    assert_eq!(
        std::fs::read_to_string(home.join("secret")).unwrap(),
        "builder's secret"
    );
    assert!(toolchain.join("tool").exists());
    assert!(!home.join("x100-probe").exists());
    assert!(!std::path::Path::new("/tmp").join(&probe).exists());
    listener.set_nonblocking(true).unwrap();
    assert!(
        listener.accept().is_err(),
        "the host's listener was reached"
    );
    sandbox.destroy().await.unwrap();
}

/// Neither of two agents' sandboxes sees the other's worktree or home.
async fn sandboxes_are_apart(backend: &dyn Backend) {
    let base = scratch();
    let (wa, wb) = (dir(base.path().join("a")), dir(base.path().join("b")));
    let a = backend
        .create(spec("apart-a", base.path(), &wa))
        .await
        .unwrap();
    let b = backend
        .create(spec("apart-b", base.path(), &wb))
        .await
        .unwrap();
    let out = run(&a, "echo a > mine && echo a > $HOME/note && echo ok").await;
    assert_eq!(stdout(&out), "ok", "{}", text(&out));
    let script = format!(
        "cat {wa}/mine {ah}/note 2>/dev/null && echo SAW; ls {base} 2>/dev/null | grep -qx a && echo LISTED; echo done",
        wa = wa.display(),
        ah = a.home().display(),
        base = base.path().display()
    );
    let out = run(&b, &script).await;
    assert_eq!(stdout(&out), "done", "{}", text(&out));
    a.destroy().await.unwrap();
    b.destroy().await.unwrap();
}

/// Git works on the worktree from inside, and merging the branch brings the work into the
/// project. Another agent's worktree stays out of sight.
async fn git_commits_inside_and_merges_outside(backend: &dyn Backend) {
    let base = scratch();
    let project = dir(base.path().join("project"));
    std::fs::write(project.join("README"), "a project\n").unwrap();
    let git = Git::new(&project);
    git.prepare().unwrap();
    let (agent, other) = (AgentId::from_u128(1), AgentId::from_u128(2));
    let branch = format!("x100/{agent}");
    let wt = git.create_worktree(agent, &branch, "main").unwrap();
    let theirs = git
        .create_worktree(other, &format!("x100/{other}"), "main")
        .unwrap();
    let mut spec = spec("git", base.path(), &wt);
    spec.paths = git_paths(&wt).unwrap();
    let sandbox = backend.create(spec).await.unwrap();
    let script = format!(
        "test -e {theirs}/README && echo SAW-OTHER
         echo 'from the agent' > feature.txt && git add -A &&
         git -c user.name=agent -c user.email=agent@x100 commit -qm 'agent work' && git log --oneline | head -1",
        theirs = theirs.display()
    );
    let out = run(&sandbox, &script).await;
    assert!(stdout(&out).ends_with("agent work"), "{}", text(&out));
    assert!(!stdout(&out).contains("SAW-OTHER"), "{}", text(&out));
    let merged = git.merge(&project, &branch).unwrap();
    assert!(matches!(merged, MergeOutcome::Merged { .. }), "{merged:?}");
    assert_eq!(
        std::fs::read_to_string(project.join("feature.txt")).unwrap(),
        "from the agent\n"
    );
    sandbox.destroy().await.unwrap();
}

/// A new driver finds the files the last one left, and the server it started through the
/// sandbox is still serving.
async fn next_driver_finds_files_and_server(backend: &dyn Backend) {
    let Some(node) = program_paths("node") else {
        panic!("node is not installed on this machine");
    };
    let node_bin = node[0].join("bin");
    let base = scratch();
    let work = dir(base.path().join("work"));
    let mut spec = spec("handoff", base.path(), &work);
    spec.programs = node;
    let sandbox = backend.create(spec).await.unwrap();
    let mut env = base_env(&sandbox);
    env[0].1 = format!("{}:/usr/bin:/bin", node_bin.display());

    let out = run_env(
        &sandbox,
        "echo left > left.txt && echo noted > $HOME/note && echo ok",
        &env,
    )
    .await;
    assert_eq!(stdout(&out), "ok", "{}", text(&out));
    let server = sandbox
        .start_process(
            sh("exec node -e \"require('http').createServer((q,s)=>s.end('still serving')).listen(18080,'127.0.0.1')\""),
            env.clone(),
        )
        .await
        .unwrap();
    assert!(
        sandbox
            .processes()
            .iter()
            .any(|p| p.id == server && p.running)
    );

    let fetch = "for i in $(seq 100); do curl -s http://127.0.0.1:18080/ && exit 0; sleep 0.1; done; exit 1";
    let out = run_env(&sandbox, &format!("cat left.txt $HOME/note; {fetch}"), &env).await;
    assert_eq!(stdout(&out), "left\nnoted\nstill serving", "{}", text(&out));

    sandbox.stop_process(server).await.unwrap();
    assert!(sandbox.processes().is_empty());
    let out = run(&sandbox, "curl -s http://127.0.0.1:18080/ || echo stopped").await;
    assert_eq!(stdout(&out), "stopped", "{}", text(&out));
    sandbox.destroy().await.unwrap();
}

/// Stopping a driver the way the drivers crate does (SIGTERM to its process group) ends a
/// background command it started in a session of its own.
async fn stopping_a_driver_ends_its_background(backend: &dyn Backend) {
    let base = scratch();
    let work = dir(base.path().join("work"));
    let sandbox = backend
        .create(spec("stop", base.path(), &work))
        .await
        .unwrap();
    let mut child = spawn(
        &sandbox,
        "setsid sh -c 'while :; do echo x >> beat; sleep 0.05; done' & sleep 1000",
        &base_env(&sandbox),
    );
    let pgid = child.id().unwrap();
    assert_stops(&work.join("beat"), async {
        nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pgid as i32),
            nix::sys::signal::Signal::SIGTERM,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("the driver ends")
            .unwrap();
    })
    .await;
    sandbox.destroy().await.unwrap();
}

/// Destroying the sandbox ends its processes and a driver's, a double fork into a session of its
/// own included.
async fn destroy_ends_everything(backend: &dyn Backend) {
    let base = scratch();
    let work = dir(base.path().join("work"));
    let sandbox = backend
        .create(spec("destroy", base.path(), &work))
        .await
        .unwrap();
    let double_fork = |file: &str| {
        format!("setsid sh -c '(while :; do echo x >> {file}; sleep 0.05; done) &'; sleep 1000")
    };
    sandbox
        .start_process(sh(&double_fork("owned")), base_env(&sandbox))
        .await
        .unwrap();
    let _driver = spawn(&sandbox, &double_fork("driven"), &base_env(&sandbox));
    grows(&work.join("driven")).await;
    assert_stops(&work.join("owned"), async {
        sandbox.destroy().await.unwrap();
    })
    .await;
    let driven = std::fs::metadata(work.join("driven")).unwrap().len();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        std::fs::metadata(work.join("driven")).unwrap().len(),
        driven
    );
}

/// A tiny HTTP server: answers every request with `body` and sends back the head it got.
async fn server(body: &'static str) -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&head).into_owned());
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            });
        }
    });
    (addr, rx)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// The proxy and the tools endpoint, and one more address mapped later, reach the servers on
/// the host, with the `Host` header as the client sent it.
async fn endpoints_reach_the_host(backend: &dyn Backend) {
    let (proxy, mut proxied) = server("proxied").await;
    let (tools, mut tooled) = server("tooled").await;
    let (hooks, mut hooked) = server("hooked").await;
    let base = scratch();
    let work = dir(base.path().join("work"));
    let mut spec = spec("endpoints", base.path(), &work);
    spec.proxy = Some(proxy);
    spec.daemon = Some(tools);
    let sandbox = backend.create(spec).await.unwrap();
    sandbox.forward(hooks).await.unwrap();
    let script = format!(
        "curl -s http://example.invalid/hello; echo; curl -s http://{tools}/drivers/d1/mcp; echo; curl -s -X POST http://{hooks}/hook/Stop"
    );
    let out = run(&sandbox, &script).await;
    assert_eq!(stdout(&out), "proxied\ntooled\nhooked", "{}", text(&out));
    let head = proxied.recv().await.unwrap();
    assert!(
        head.starts_with("GET http://example.invalid/hello HTTP/1.1"),
        "{head}"
    );
    let head = tooled.recv().await.unwrap();
    assert_eq!(
        header(&head, "host"),
        Some(tools.to_string().as_str()),
        "{head}"
    );
    let head = hooked.recv().await.unwrap();
    assert!(head.starts_with("POST /hook/Stop"), "{head}");
    sandbox.destroy().await.unwrap();
}

/// A lab sandbox, with no proxy, has no way out, and nothing listens where a proxy would be.
async fn lab_has_no_way_out(backend: &dyn Backend) {
    let (proxy, _) = server("proxied").await;
    let base = scratch();
    let work = dir(base.path().join("work"));
    let sandbox = backend
        .create(spec("lab", base.path(), &work))
        .await
        .unwrap();
    let script = format!(
        "env | grep -i _proxy; curl -s -m 5 http://example.invalid/ || echo no-route; curl -s -m 5 -x http://{proxy} http://example.invalid/ || echo no-proxy"
    );
    let out = run(&sandbox, &script).await;
    assert_eq!(stdout(&out), "no-route\nno-proxy", "{}", text(&out));
    sandbox.destroy().await.unwrap();
}

/// A named read-write path is writable through a link from a home inside, as a terminal
/// driver's login is: a whole directory linked (Claude Code) and one file linked (Codex), each
/// rewritten by rename as the programs do on a token refresh.
async fn login_link_is_writable(backend: &dyn Backend) {
    let base = scratch();
    let login = dir(base.path().join("stand-in-login"));
    std::fs::write(login.join("auth.json"), "old").unwrap();
    let driver = dir(base.path().join("driver"));
    let home = dir(driver.join("home"));
    let codex_home = dir(driver.join("codex"));
    std::os::unix::fs::symlink(&login, home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(login.join("auth.json"), codex_home.join("auth.json")).unwrap();
    let work = dir(base.path().join("work"));
    let mut spec = spec("login", base.path(), &work);
    spec.paths = vec![Mount::read_write(&login), Mount::read_write(&driver)];
    let sandbox = backend.create(spec).await.unwrap();
    let mut env = base_env(&sandbox);
    env[1].1 = home.display().to_string();
    let script = format!(
        "echo claude > $HOME/.claude/auth.json.tmp && mv $HOME/.claude/auth.json.tmp $HOME/.claude/auth.json &&
         real=$(readlink -f {codex}/auth.json) && echo codex > $real.tmp && mv $real.tmp $real && echo ok",
        codex = codex_home.display()
    );
    let out = run_env(&sandbox, &script, &env).await;
    assert_eq!(stdout(&out), "ok", "{}", text(&out));
    assert_eq!(
        std::fs::read_to_string(login.join("auth.json")).unwrap(),
        "codex\n"
    );
    sandbox.destroy().await.unwrap();
}

/// The real `claude` and `codex` start inside far enough to print their versions. No login is
/// visible: the home is the sandbox's own.
async fn real_harnesses_start(backend: &dyn Backend) {
    let (Some(claude), Some(codex)) = (program_paths("claude"), program_paths("codex")) else {
        panic!("claude or codex is not installed on this machine");
    };
    let base = scratch();
    let work = dir(base.path().join("work"));
    let mut spec = spec("harnesses", base.path(), &work);
    let path = format!(
        "{}:{}/bin:/usr/bin:/bin",
        claude[0].parent().unwrap().display(),
        codex[0].display()
    );
    spec.programs = claude.into_iter().chain(codex).collect();
    let sandbox = backend.create(spec).await.unwrap();
    let mut env = base_env(&sandbox);
    env[0].1 = path;
    env.push(("DISABLE_AUTOUPDATER".into(), "1".into()));
    let out = run_env(&sandbox, "claude --version && codex --version", &env).await;
    let said = stdout(&out);
    assert!(
        said.contains("(Claude Code)") && said.contains("codex"),
        "{}",
        text(&out)
    );
    eprintln!("inside: {said}");
    sandbox.destroy().await.unwrap();
}

/// A driver started in a pseudo-terminal, as a terminal driver is, has a terminal inside. The
/// host's `script` stands in for the drivers crate's terminal.
async fn terminal_reaches_inside(backend: &dyn Backend) {
    let base = scratch();
    let work = dir(base.path().join("work"));
    let sandbox = backend
        .create(spec("tty", base.path(), &work))
        .await
        .unwrap();
    let line = sandbox
        .wrap()
        .apply(sh("test -t 0 && test -t 1 && echo has-a-terminal"));
    let quoted: Vec<String> = std::iter::once(line.program.into_os_string())
        .chain(line.args)
        .map(|a| format!("'{}'", a.to_string_lossy().replace('\'', "'\\''")))
        .collect();
    let out = tokio::process::Command::new("script")
        .args(["-qec", &quoted.join(" "), "/dev/null"])
        .env_clear()
        .envs(base_env(&sandbox))
        .current_dir(&work)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(stdout(&out).contains("has-a-terminal"), "{}", text(&out));
    sandbox.destroy().await.unwrap();
}

/// A Unix socket made inside, in a mapped directory, takes connections from the host, as Codex's
/// app-server's does from the daemon.
async fn host_reaches_a_socket_inside(backend: &dyn Backend) {
    let Some(node) = program_paths("node") else {
        panic!("node is not installed on this machine");
    };
    let base = scratch();
    let work = dir(base.path().join("work"));
    let mut spec = spec("socket", base.path(), &work);
    let node_bin = node[0].join("bin");
    spec.programs = node;
    let sandbox = backend.create(spec).await.unwrap();
    let mut env = base_env(&sandbox);
    env[0].1 = format!("{}:/usr/bin:/bin", node_bin.display());
    let socket = work.join("app.sock");
    let serve = format!(
        "exec node -e \"require('net').createServer(s=>s.end('from inside')).listen('{}')\"",
        socket.display()
    );
    sandbox.start_process(sh(&serve), env).await.unwrap();
    let mut said = String::new();
    for _ in 0..100 {
        if let Ok(mut s) = tokio::net::UnixStream::connect(&socket).await {
            s.read_to_string(&mut said).await.unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(said, "from inside");
    sandbox.destroy().await.unwrap();
}
