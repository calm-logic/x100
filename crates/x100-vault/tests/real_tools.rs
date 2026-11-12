//! Real programs through the proxy: `curl`, and the real `claude` and `codex` binaries with a
//! reference as their API key, against the drivers crate's fake provider behind TLS from a test
//! authority. Nothing reaches a real provider: the base URL is a made-up host the proxy sends to
//! loopback, the proxy trusts only the test authority upstream, the hosts these programs are
//! known to reach otherwise are refused, the environment is built from nothing, the key is a
//! dummy, and every report is checked to name only the fake provider.
//!
//! These settle what HARNESSES.md left open: whether each harness's own authority setting
//! (`NODE_EXTRA_CA_CERTS`, `CODEX_CA_CERTIFICATE`) makes it trust the proxy, and whether each
//! sends the route's token as `Proxy-Authorization`.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use x100_core::credential::{Projects, UseOutcome};
use x100_core::driver::Purpose;
use x100_core::effort::Effort;
use x100_core::harness::ModelName;
use x100_core::id::DriverId;
use x100_core::time::{FakeClock, Timestamp};
use x100_drivers::claude::{ClaudeCode, ClaudeCodeConfig};
use x100_drivers::codex::{Codex, CodexConfig};
use x100_drivers::testing::{FakeProvider, Reply};
use x100_drivers::{
    CommandWrapper, DriverOutput, DriverSpec, Expose, Harness, Permissions, SpawnHook, StallPolicy,
    TurnEnd,
};
use x100_vault::{EgressOutcome, sandbox_env};

/// Hosts the two programs are known to reach besides their provider. Refused, so that even a
/// change in either program cannot open a connection to them from a test.
const ELSEWHERE: &[&str] = &[
    "api.anthropic.com",
    "statsig.anthropic.com",
    "console.anthropic.com",
    "mcp-proxy.anthropic.com",
    "claude.ai",
    "sentry.io",
    "api.openai.com",
    "chatgpt.com",
    "ab.chatgpt.com",
    "github.com",
    "api.github.com",
    "raw.githubusercontent.com",
];

fn installed(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file());
    if found.is_none() {
        eprintln!("SKIPPED: {program} is not installed on this machine");
    }
    found
}

#[tokio::test]
async fn curl_goes_through_the_proxy_trusting_only_its_authority() {
    let Some(curl) = installed("curl") else {
        return;
    };
    let mut s = setup();
    let server = TestServer::https(
        Some(s.ca.server_config(&[ALLOWED])),
        |req: http::Request<hyper::body::Incoming>| async move {
            let auth = req
                .headers()
                .get("authorization")
                .map(|v| v.as_bytes().to_vec())
                .unwrap_or_default();
            if auth == format!("Bearer {VALUE}").as_bytes() {
                respond(200, "deployed")
            } else {
                respond(401, "who are you")
            }
        },
    )
    .await;
    let route = s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let authority = s.vault.authority().unwrap();
    let out = tokio::process::Command::new(curl)
        .env_clear()
        .envs(sandbox_env(&route.url(), authority.certificate_path(), &[]))
        .args(["-sS", "-H"])
        .arg(format!("Authorization: Bearer {}", s.reference))
        .arg(format!("https://{ALLOWED}:{}/deploy", server.port))
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "deployed");
    assert_no_leak("curl's output", &[out.stdout, out.stderr].concat());
    let report = s.report().await;
    assert_eq!(report.uses, vec![(s.reference, UseOutcome::Injected)]);

    // A client that sends its credentials only after a challenge, as git does, gets one.
    let out = tokio::process::Command::new("curl")
        .env_clear()
        .args([
            "-sS",
            "--proxy-anyauth",
            "--proxy",
            &route.url(),
            "--cacert",
        ])
        .arg(authority.certificate_path())
        .args(["-H", &format!("Authorization: Bearer {}", s.reference)])
        .arg(format!("https://{ALLOWED}:{}/deploy", server.port))
        .output()
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "deployed",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    s.report().await;

    // Without the authority, curl refuses the proxy's certificate: nothing else trusts it.
    let out = tokio::process::Command::new("curl")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HTTPS_PROXY", route.url())
        .env("CURL_CA_BUNDLE", s.ca_file())
        .args(["-sS", &format!("https://{ALLOWED}:{}/deploy", server.port)])
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("certificate"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Setup {
    /// The test upstream authority as a file, standing in for some other trust store.
    fn ca_file(&self) -> PathBuf {
        let path = self.dir.path().join("test-upstream-ca.pem");
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &self.ca.cert)
        );
        std::fs::write(&path, pem).unwrap();
        path
    }
}

/// The fake provider behind TLS for `provider.x100.test`, as a real provider would be.
struct TlsFront {
    port: u16,
    task: JoinHandle<()>,
}

impl Drop for TlsFront {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn tls_front(ca: &TestCa, provider: &FakeProvider) -> TlsFront {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(ca.server_config(&[PROVIDER]));
    let backend = provider.url().trim_start_matches("http://").to_owned();
    let task = tokio::spawn(async move {
        let mut conns = tokio::task::JoinSet::new();
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, backend) = (acceptor.clone(), backend.clone());
            conns.spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let Ok(mut plain) = TcpStream::connect(backend).await else {
                    return;
                };
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut plain).await;
            });
        }
    });
    TlsFront { port, task }
}

fn spec(dir: &std::path::Path, model: &str, env: Vec<(String, String)>) -> DriverSpec {
    DriverSpec {
        driver: DriverId::from_u128(90),
        model: ModelName(model.to_owned()),
        effort: Effort::Low,
        dir: dir.to_owned(),
        // Beside the worktree, in the test's own directory.
        state: dir.with_file_name("driver"),
        wrap: CommandWrapper::identity(),
        expose: Expose::nothing(),
        tmp: None,
        start_pack: "You are a test agent.".into(),
        tools: None,
        env,
        stall: StallPolicy {
            silent: Duration::from_secs(300),
            while_tool_runs: Duration::from_secs(1800),
            check_every: Duration::from_secs(3600),
        },
        permissions: Permissions::AllowAll,
        on_spawn: SpawnHook::ignore(),
        login: None,
    }
}

struct Run {
    s: Setup,
    provider: FakeProvider,
    _front: TlsFront,
    dirs: tempfile::TempDir,
}

async fn prepare() -> Run {
    let s = setup_with(&[PROVIDER], Projects::All, None);
    for host in ELSEWHERE {
        s.proxy.refuse_host(host);
    }
    let provider = FakeProvider::start().await.unwrap();
    let front = tls_front(&s.ca, &provider).await;
    let dirs = tempfile::tempdir().unwrap();
    for d in ["work", "home", "state"] {
        std::fs::create_dir(dirs.path().join(d)).unwrap();
    }
    Run {
        s,
        provider,
        _front: front,
        dirs,
    }
}

impl Run {
    fn path(&self, name: &str) -> PathBuf {
        self.dirs.path().join(name)
    }

    /// Drains the reports and checks that each was for the fake provider and swapped the value
    /// in, and that nothing else was asked for.
    fn check_reports(&mut self) -> usize {
        let mut n = 0;
        while let Ok(report) = self.s.reports.try_recv() {
            assert_eq!(report.host, PROVIDER, "{report:?}");
            assert!(
                matches!(report.outcome, EgressOutcome::Answered(_)),
                "{report:?}"
            );
            assert!(
                report
                    .uses
                    .iter()
                    .all(|u| *u == (self.s.reference, UseOutcome::Injected)),
                "{report:?}"
            );
            n += 1;
        }
        n
    }
}

async fn one_turn(handle: &mut x100_drivers::DriverHandle) -> Vec<DriverOutput> {
    handle.turn("Say done.").await.unwrap();
    let mut seen = Vec::new();
    loop {
        let out = tokio::time::timeout(Duration::from_secs(120), handle.next())
            .await
            .expect("the driver said nothing for too long")
            .expect("the driver's stream ended");
        let end = matches!(out, DriverOutput::TurnEnded(_) | DriverOutput::Failed(_));
        seen.push(out);
        if end {
            return seen;
        }
    }
}

#[tokio::test]
async fn claude_code_with_a_reference_for_its_key_reaches_the_provider_with_the_value() {
    let Some(program) = installed("claude") else {
        return;
    };
    let mut run = prepare().await;
    run.provider.reply(Reply::Text("done".into()));
    let route = run
        .s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let authority = run.s.vault.authority().unwrap();
    let mut config = ClaudeCodeConfig::new();
    config.program = program;
    config.base_url = Some(format!("https://{PROVIDER}:{}", run._front.port));
    config.inherit_env = vec!["PATH".into()];
    let harness = ClaudeCode::new(config, Arc::new(FakeClock::new(Timestamp::from_millis(1))));
    let env = vec![
        ("HOME".into(), run.path("home").display().to_string()),
        ("ANTHROPIC_API_KEY".into(), run.s.reference.to_string()),
        ("HTTPS_PROXY".into(), route.url()),
        (
            "NODE_EXTRA_CA_CERTS".into(),
            authority.certificate_path().display().to_string(),
        ),
    ];
    let mut handle = harness
        .start(spec(&run.path("work"), "claude-haiku-5-5", env))
        .await
        .unwrap();
    let outputs = one_turn(&mut handle).await;
    assert!(
        matches!(
            outputs.last(),
            Some(DriverOutput::TurnEnded(TurnEnd::Completed))
        ),
        "{outputs:#?}"
    );
    assert!(
        outputs.contains(&DriverOutput::Text("done".into())),
        "{outputs:#?}"
    );
    handle.stop().await;
    let requests = run.provider.requests();
    assert!(!requests.is_empty());
    for r in &requests {
        assert_eq!(
            r.header("x-api-key"),
            Some(VALUE),
            "the provider got the value"
        );
    }
    assert_no_leak("the driver's output", format!("{outputs:?}").as_bytes());
    assert!(run.check_reports() >= 1);
}

#[tokio::test]
async fn codex_with_a_reference_for_its_key_reaches_the_provider_with_the_value() {
    let Some(program) = installed("codex") else {
        return;
    };
    let mut run = prepare().await;
    run.provider.reply(Reply::Text("done".into()));
    let route = run
        .s
        .proxy
        .open_route(requester(Purpose::Work), &[])
        .await
        .unwrap();
    let authority = run.s.vault.authority().unwrap();
    let mut config = CodexConfig::new();
    config.program = program;
    config.base_url = format!("https://{PROVIDER}:{}/v1", run._front.port);
    config.inherit_env = vec!["PATH".into()];
    let harness = Codex::new(config, Arc::new(FakeClock::new(Timestamp::from_millis(1))));
    let env = vec![
        ("HOME".into(), run.path("home").display().to_string()),
        ("OPENAI_API_KEY".into(), run.s.reference.to_string()),
        ("HTTPS_PROXY".into(), route.url()),
        (
            "CODEX_CA_CERTIFICATE".into(),
            authority.certificate_path().display().to_string(),
        ),
    ];
    let mut handle = harness
        .start(spec(&run.path("work"), "gpt-6-luna", env))
        .await
        .unwrap();
    let outputs = one_turn(&mut handle).await;
    assert!(
        matches!(
            outputs.last(),
            Some(DriverOutput::TurnEnded(TurnEnd::Completed))
        ),
        "{outputs:#?}"
    );
    handle.stop().await;
    let requests = run.provider.requests();
    assert!(!requests.is_empty());
    for r in &requests {
        assert_eq!(
            r.header("authorization"),
            Some(format!("Bearer {VALUE}").as_str())
        );
    }
    assert_no_leak("the driver's output", format!("{outputs:?}").as_bytes());
    assert!(run.check_reports() >= 1);
}
