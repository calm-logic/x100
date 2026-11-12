//! The wrapper runs one named command with the value in that child's environment only, and the
//! value never comes back out in its output.

mod common;

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{VALUE, assert_no_leak, requester};
use x100_core::credential::{Credential, CredentialScope, Projects, Refusal, UseOutcome, Via};
use x100_core::driver::Purpose;
use x100_core::id::{CredentialRef, ProjectId, TrialId};
use x100_core::trial::Arm;
use x100_vault::{NoKeychain, SecretValue, Vault, WrapError, WrapRequest, Wrapper};

const VAR: &str = "HOMEBOX_TOKEN";

/// A stand-in deploy tool that is careless with its credential in every way it can be.
const TOOL: &str = r#"#!/bin/sh
printf 'raw:%s\n' "$HOMEBOX_TOKEN"
printf '%s' "$HOMEBOX_TOKEN" | base64 -w0; echo
printf '%s' "$HOMEBOX_TOKEN" | od -An -tx1 | tr -d ' \n'; echo
printf 'split:%s' "$(printf '%s' "$HOMEBOX_TOKEN" | cut -c1-10)"
sleep 0.2
printf '%s\n' "$(printf '%s' "$HOMEBOX_TOKEN" | cut -c11-)"
printf 'err:%s\n' "$HOMEBOX_TOKEN" >&2
echo "args:$*"
echo "dir:$(pwd)"
exit 3
"#;

struct Setup {
    _dir: tempfile::TempDir,
    vault: Vault,
    reference: CredentialRef,
    tools: PathBuf,
    worktree: PathBuf,
}

fn install(dir: &Path, name: &str) {
    let path = dir.join(name);
    std::fs::write(&path, TOOL).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn setup(projects: Projects) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("vault"), &NoKeychain).unwrap();
    let reference = CredentialRef::from_u128(0x5eed);
    let commands = ["deploytool", "wrangler", "bash", "homebox"].map(str::to_owned);
    vault
        .add(
            Credential {
                reference,
                name: "homebox".into(),
                scope: CredentialScope {
                    hosts: BTreeSet::new(),
                    commands: BTreeSet::from(commands),
                    projects,
                },
            },
            SecretValue::from(VALUE),
        )
        .unwrap();
    let tools = dir.path().join("tools");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::create_dir_all(worktree.join("bin")).unwrap();
    install(&tools, "deploytool");
    install(&tools, "wrangler");
    install(&worktree.join("bin"), "homebox");
    Setup {
        _dir: dir,
        vault,
        reference,
        tools,
        worktree,
    }
}

impl Setup {
    fn request(&self, command: &str, changed: &[&str]) -> WrapRequest {
        WrapRequest {
            requester: requester(Purpose::Work),
            credential: self.reference,
            env_var: VAR.into(),
            command: command.into(),
            args: vec!["deploy".into(), "--app".into(), "site".into()],
            dir: self.worktree.clone(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            search_path: vec![self.tools.clone(), self.worktree.join("bin")],
            writable: vec![self.worktree.clone()],
            changed: changed.iter().map(PathBuf::from).collect(),
        }
    }
}

fn refusal(result: Result<x100_vault::WrapOutput, WrapError>) -> Refusal {
    match result {
        Err(WrapError::Refused(used)) => match used.outcome {
            UseOutcome::Refused(r) => r,
            UseOutcome::Injected => panic!("a refusal recorded as injected"),
        },
        Err(e) => panic!("not a refusal: {e}"),
        Ok(_) => panic!("ran"),
    }
}

#[tokio::test]
async fn the_named_command_gets_the_value_and_its_output_comes_back_redacted() {
    let s = setup(Projects::All);
    let wrapper = Wrapper::new(s.vault.clone());
    let out = wrapper
        .run(s.request("deploytool", &["src/app.ts"]))
        .await
        .unwrap();
    assert_eq!(out.status, Some(3));
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let stderr = String::from_utf8(out.stderr.clone()).unwrap();
    let reference = s.reference.to_string();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], format!("raw:{reference}"));
    assert!(lines[1].contains(&reference), "base64: {}", lines[1]);
    assert_eq!(lines[2], reference, "hex");
    assert_eq!(lines[3], format!("split:{reference}"));
    assert_eq!(lines[4], "args:deploy --app site");
    assert_eq!(lines[5], format!("dir:{}", s.worktree.display()));
    assert_eq!(stderr, format!("err:{reference}\n"));
    assert_no_leak("stdout", &out.stdout);
    assert_no_leak("stderr", &out.stderr);
    assert_eq!(out.used.outcome, UseOutcome::Injected);
    assert_eq!(
        out.used.via,
        Via::Wrapper {
            command: "deploytool".into()
        }
    );
    // The value went to the child only.
    assert!(std::env::var_os(VAR).is_none());
}

#[tokio::test]
async fn commands_off_the_list_and_general_runners_are_refused() {
    let s = setup(Projects::All);
    let wrapper = Wrapper::new(s.vault.clone());
    assert_eq!(
        refusal(wrapper.run(s.request("curl", &[])).await),
        Refusal::CommandNotAllowed
    );
    // On the credential's list, but a shell runs whatever the agent writes.
    assert_eq!(
        refusal(wrapper.run(s.request("bash", &[])).await),
        Refusal::CommandNotAllowed
    );
    // On the list, but found in a directory the agent can write to.
    assert_eq!(
        refusal(wrapper.run(s.request("homebox", &[])).await),
        Refusal::CommandNotAllowed
    );
    for bad in ["../tools/deploytool", "/bin/sh", ""] {
        assert!(matches!(
            wrapper.run(s.request(bad, &[])).await,
            Err(WrapError::BadRequest(_))
        ));
    }
    let mut unknown = s.request("deploytool", &[]);
    unknown.credential = CredentialRef::from_u128(1);
    assert!(matches!(
        wrapper.run(unknown).await,
        Err(WrapError::Unknown(_))
    ));
}

#[tokio::test]
async fn a_tool_is_refused_while_the_agents_changes_touch_its_config() {
    let s = setup(Projects::All);
    let wrapper =
        Wrapper::new(s.vault.clone()).with_config_files("deploytool", ["deploy.yaml".to_owned()]);
    assert_eq!(
        refusal(
            wrapper
                .run(s.request("wrangler", &["src/a.ts", "site/wrangler.toml"]))
                .await
        ),
        Refusal::ConfigTouched
    );
    assert_eq!(
        refusal(wrapper.run(s.request("wrangler", &["package.json"])).await),
        Refusal::ConfigTouched
    );
    assert!(
        wrapper
            .run(s.request("wrangler", &["src/a.ts"]))
            .await
            .is_ok()
    );
    assert_eq!(
        refusal(
            wrapper
                .run(s.request("deploytool", &["ops/deploy.yaml"]))
                .await
        ),
        Refusal::ConfigTouched
    );
}

#[tokio::test]
async fn a_lab_driver_and_a_project_not_allowed_get_nothing() {
    let s = setup(Projects::All);
    let wrapper = Wrapper::new(s.vault.clone());
    let mut lab = s.request("deploytool", &[]);
    lab.requester = requester(Purpose::Lab {
        trial: TrialId::from_u128(1),
        arm: Arm::Baseline,
        run: 0,
    });
    assert_eq!(refusal(wrapper.run(lab).await), Refusal::LabDriver);

    let s = setup(Projects::Only(BTreeSet::from([ProjectId::from_u128(1)])));
    let wrapper = Wrapper::new(s.vault.clone());
    assert_eq!(
        refusal(wrapper.run(s.request("deploytool", &[])).await),
        Refusal::ProjectNotAllowed
    );
}
