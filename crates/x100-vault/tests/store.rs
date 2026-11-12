//! The store on disk: what one run writes the next reads, the file never holds a value in any
//! form, and a key that is wrong or missing is an error, never an empty vault.

mod common;

use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;

use common::{assert_no_leak, scope};
use x100_core::credential::{Credential, Projects};
use x100_core::id::CredentialRef;
use x100_vault::{KeySource, Keychain, NoKeychain, SecretValue, Unavailable, Vault, VaultError};
use zeroize::Zeroizing;

/// A dummy value for these tests only.
const VALUE: &str = common::VALUE;
const OTHER_VALUE: &str = "second-dummy-value-0123456789";

fn credential(n: u128, hosts: &[&str]) -> Credential {
    Credential {
        reference: CredentialRef::from_u128(n),
        name: format!("cred {n}"),
        scope: scope(hosts, Projects::All),
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Stands in for the OS keychain so no test touches the builder's own.
#[derive(Default)]
struct StandIn(Mutex<HashMap<String, Vec<u8>>>);

impl Keychain for StandIn {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, Unavailable> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .map(Zeroizing::new))
    }

    fn set(&self, account: &str, secret: &[u8]) -> Result<(), Unavailable> {
        self.0
            .lock()
            .unwrap()
            .insert(account.to_owned(), secret.to_vec());
        Ok(())
    }
}

#[test]
fn what_one_run_writes_the_next_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    {
        let vault = Vault::open(&path, &NoKeychain).unwrap();
        assert_eq!(vault.key_source(), KeySource::File);
        vault
            .add(credential(1, &["a.test"]), SecretValue::from(VALUE))
            .unwrap();
        vault
            .add(
                credential(2, &["b.test"]),
                SecretValue::from("to-be-removed-123"),
            )
            .unwrap();
        vault
            .add(
                credential(3, &["c.test"]),
                SecretValue::from("old-value-1234567"),
            )
            .unwrap();
        vault
            .replace(CredentialRef::from_u128(3), SecretValue::from(OTHER_VALUE))
            .unwrap();
        vault
            .set_scope(
                CredentialRef::from_u128(1),
                scope(&["a.test", "a2.test"], Projects::All),
            )
            .unwrap();
        vault.remove(CredentialRef::from_u128(2)).unwrap();
    }
    let vault = Vault::open(&path, &NoKeychain).unwrap();
    let listed = vault.list();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed[0].scope.hosts,
        BTreeSet::from(["a.test".to_owned(), "a2.test".to_owned()])
    );
    assert_eq!(vault.get(CredentialRef::from_u128(2)), None);
    // The values came back too: the redactor knows them.
    let redactor = vault.redactor();
    assert_eq!(
        redactor.redact_str(&format!("{VALUE} {OTHER_VALUE}")),
        format!(
            "{} {}",
            CredentialRef::from_u128(1),
            CredentialRef::from_u128(3)
        )
    );
    assert_eq!(
        redactor.redact_str("old-value-1234567"),
        "old-value-1234567"
    );
}

#[test]
fn the_files_on_disk_hold_no_value_and_the_key_file_is_the_owners_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let vault = Vault::open(&path, &NoKeychain).unwrap();
    vault
        .add(credential(1, &["a.test"]), SecretValue::from(VALUE))
        .unwrap();
    let authority = vault.authority().unwrap();
    assert_eq!(mode(&path), 0o700);
    assert_eq!(mode(&path.join("vault.key")), 0o600);
    assert_eq!(mode(&path.join("vault.sealed")), 0o600);
    assert_eq!(mode(&path.join("authority.key.sealed")), 0o600);
    assert_eq!(mode(authority.certificate_path()), 0o644);
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&path).unwrap() {
        let entry = entry.unwrap();
        let bytes = std::fs::read(entry.path()).unwrap();
        assert_no_leak(&entry.path().display().to_string(), &bytes);
        assert!(
            memchr::memmem::find(&bytes, b"PRIVATE KEY").is_none(),
            "{} holds a private key in the clear",
            entry.path().display()
        );
        names.push(entry.file_name().into_string().unwrap());
    }
    names.sort();
    assert_eq!(
        names,
        [
            "authority.key.sealed",
            "authority.pem",
            "vault.key",
            "vault.sealed"
        ]
    );
}

#[test]
fn a_wrong_key_is_an_error_not_an_empty_vault() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    Vault::open(&path, &NoKeychain)
        .unwrap()
        .add(credential(1, &["a.test"]), SecretValue::from(VALUE))
        .unwrap();
    std::fs::write(path.join("vault.key"), [7u8; 32]).unwrap();
    let err = Vault::open(&path, &NoKeychain).err().expect("refused");
    assert!(matches!(err, VaultError::WrongKey(_)), "{err}");
    assert!(err.to_string().contains("does not open"));
}

#[test]
fn a_missing_key_file_is_an_error_not_an_empty_vault() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    Vault::open(&path, &NoKeychain)
        .unwrap()
        .add(credential(1, &["a.test"]), SecretValue::from(VALUE))
        .unwrap();
    std::fs::remove_file(path.join("vault.key")).unwrap();
    let err = Vault::open(&path, &NoKeychain).err().expect("refused");
    assert!(matches!(err, VaultError::KeyMissing(_)), "{err}");
    assert!(!path.join("vault.key").exists(), "no new key was made");
}

#[test]
fn a_key_file_others_can_read_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    drop(Vault::open(&path, &NoKeychain).unwrap());
    std::fs::set_permissions(
        path.join("vault.key"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let err = Vault::open(&path, &NoKeychain).err().expect("refused");
    assert!(matches!(err, VaultError::KeyFileExposed(_, 0o644)), "{err}");
}

#[test]
fn a_changed_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    Vault::open(&path, &NoKeychain)
        .unwrap()
        .add(credential(1, &["a.test"]), SecretValue::from(VALUE))
        .unwrap();
    let file = path.join("vault.sealed");
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&file, bytes).unwrap();
    assert!(matches!(
        Vault::open(&path, &NoKeychain),
        Err(VaultError::WrongKey(_))
    ));
}

#[test]
fn with_a_keychain_the_key_lives_there_and_nowhere_in_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let keychain = StandIn::default();
    let vault = Vault::open(&path, &keychain).unwrap();
    assert_eq!(vault.key_source(), KeySource::Keychain);
    vault
        .add(credential(1, &["a.test"]), SecretValue::from(VALUE))
        .unwrap();
    assert!(!path.join("vault.key").exists());
    let stored = keychain.0.lock().unwrap().clone();
    assert_eq!(stored.len(), 1);
    let (account, key) = stored.iter().next().unwrap();
    assert!(
        account.starts_with("vault-key ") && account.ends_with("vault"),
        "{account}"
    );
    assert_eq!(key.len(), 32);
    drop(vault);

    let reopened = Vault::open(&path, &keychain).unwrap();
    assert_eq!(reopened.list().len(), 1);
    // A keychain that has lost the entry, or cannot be reached, is an error: the store must not
    // quietly start again with a key file.
    let err = Vault::open(&path, &StandIn::default())
        .err()
        .expect("refused");
    assert!(matches!(err, VaultError::KeyMissing(_)), "{err}");
    let err = Vault::open(&path, &NoKeychain).err().expect("refused");
    assert!(matches!(err, VaultError::KeychainUnavailable(_)), "{err}");
    assert!(!path.join("vault.key").exists());
}

#[test]
fn values_too_short_to_redact_and_duplicate_references_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(&dir.path().join("vault"), &NoKeychain).unwrap();
    assert!(matches!(
        vault.add(credential(1, &[]), SecretValue::from("short")),
        Err(VaultError::TooShort(8))
    ));
    vault
        .add(credential(1, &[]), SecretValue::from(VALUE))
        .unwrap();
    assert!(matches!(
        vault.add(credential(1, &[]), SecretValue::from(VALUE)),
        Err(VaultError::Exists(_))
    ));
    assert!(matches!(
        vault.replace(CredentialRef::from_u128(9), SecretValue::from(VALUE)),
        Err(VaultError::Unknown(_))
    ));
}

#[test]
fn the_authority_is_kept_between_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault");
    let first = Vault::open(&path, &NoKeychain)
        .unwrap()
        .authority()
        .unwrap();
    let pem = std::fs::read_to_string(first.certificate_path()).unwrap();
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
    let second = Vault::open(&path, &NoKeychain)
        .unwrap()
        .authority()
        .unwrap();
    assert_eq!(first.certificate(), second.certificate());
}
