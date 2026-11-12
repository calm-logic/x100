//! The credential store: values sealed at rest, held in memory as [`SecretValue`], and handed
//! out only to the proxy and the wrapper in this crate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use x100_core::credential::{Credential, CredentialScope, Refusal, Via};
use x100_core::id::CredentialRef;
use zeroize::Zeroizing;

use crate::keychain::Keychain;
use crate::redact::{self, Patterns, Redactor};
use crate::scope::{self, Requester};
use crate::seal::{self, KeySource, Sealer};
use crate::secret::{MIN_LEN, SecretValue};
use crate::{Authority, VaultError};

const CREDENTIALS: &str = "vault.sealed";
const KIND: &str = "credentials";

struct Entry {
    credential: Credential,
    value: Arc<SecretValue>,
}

pub(crate) struct Inner {
    pub(crate) dir: PathBuf,
    pub(crate) sealer: Sealer,
    entries: RwLock<BTreeMap<CredentialRef, Entry>>,
    redaction: Arc<redact::Shared>,
}

/// The vault. Cheap to clone: every clone is the same store.
#[derive(Clone)]
pub struct Vault {
    pub(crate) inner: Arc<Inner>,
}

/// Why a credential was not handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Denied {
    /// No credential has this reference. Nothing to record: it is not a use.
    Unknown,
    Refused(Refusal),
}

impl Vault {
    /// Opens the vault in `dir`, creating the directory, the key and an empty store on first
    /// run. The key comes from `keychain` where it is available and otherwise from a key file in
    /// `dir`. A store sealed with a key that is now missing or wrong is an error. This reads
    /// files and may talk to D-Bus, so call it from a blocking context.
    pub fn open(dir: &Path, keychain: &dyn Keychain) -> Result<Self, VaultError> {
        seal::ensure_dir(dir)?;
        let path = dir.join(CREDENTIALS);
        let sealer = Sealer::unlock(dir, &path, keychain)?;
        let entries = match std::fs::read(&path) {
            Ok(bytes) => decode(&sealer.open(KIND, &path, &bytes)?, &path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                persist(&path, &sealer, &BTreeMap::new())?;
                BTreeMap::new()
            }
            Err(e) => return Err(VaultError::Io(path, e)),
        };
        let patterns = patterns(&entries);
        Ok(Self {
            inner: Arc::new(Inner {
                dir: dir.to_owned(),
                sealer,
                entries: RwLock::new(entries),
                redaction: Arc::new(redact::Shared::new(patterns)),
            }),
        })
    }

    pub fn key_source(&self) -> KeySource {
        self.inner.sealer.source()
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Adds a credential under a reference the caller chose (the daemon makes ids from its
    /// clock). Refuses a reference already in use and a value too short to redact.
    pub fn add(&self, credential: Credential, value: SecretValue) -> Result<(), VaultError> {
        if value.len() < MIN_LEN {
            return Err(VaultError::TooShort(MIN_LEN));
        }
        self.change(|entries| {
            let reference = credential.reference;
            if entries.contains_key(&reference) {
                return Err(VaultError::Exists(reference));
            }
            entries.insert(
                reference,
                Entry {
                    credential,
                    value: Arc::new(value),
                },
            );
            Ok(())
        })
    }

    /// Replaces a credential's value, keeping its reference and scope.
    pub fn replace(&self, reference: CredentialRef, value: SecretValue) -> Result<(), VaultError> {
        if value.len() < MIN_LEN {
            return Err(VaultError::TooShort(MIN_LEN));
        }
        self.change(|entries| {
            let entry = entries
                .get_mut(&reference)
                .ok_or(VaultError::Unknown(reference))?;
            entry.value = Arc::new(value);
            Ok(())
        })
    }

    pub fn set_scope(
        &self,
        reference: CredentialRef,
        scope: CredentialScope,
    ) -> Result<(), VaultError> {
        self.change(|entries| {
            let entry = entries
                .get_mut(&reference)
                .ok_or(VaultError::Unknown(reference))?;
            entry.credential.scope = scope;
            Ok(())
        })
    }

    pub fn remove(&self, reference: CredentialRef) -> Result<(), VaultError> {
        self.change(|entries| {
            entries
                .remove(&reference)
                .ok_or(VaultError::Unknown(reference))?;
            Ok(())
        })
    }

    pub fn get(&self, reference: CredentialRef) -> Option<Credential> {
        self.read().get(&reference).map(|e| e.credential.clone())
    }

    pub fn list(&self) -> Vec<Credential> {
        self.read().values().map(|e| e.credential.clone()).collect()
    }

    /// A redactor that always knows the vault's current values.
    pub fn redactor(&self) -> Redactor {
        Redactor(Arc::clone(&self.inner.redaction))
    }

    /// The certificate authority the proxy ends TLS with, created in the vault's directory on
    /// first call, its private key sealed like a value.
    pub fn authority(&self) -> Result<Authority, VaultError> {
        Authority::load_or_create(&self.inner.dir, &self.inner.sealer)
    }

    /// The value for one use, after checking the use against the credential's scope. Only the
    /// proxy and the wrapper call this.
    pub(crate) fn take(
        &self,
        reference: CredentialRef,
        requester: &Requester,
        via: &Via,
    ) -> Result<Arc<SecretValue>, Denied> {
        let entries = self.read();
        let entry = entries.get(&reference).ok_or(Denied::Unknown)?;
        scope::check(&entry.credential, requester, via).map_err(Denied::Refused)?;
        Ok(Arc::clone(&entry.value))
    }

    /// The value for a request the daemon makes itself to `host`. Only [`crate::Proxy::call`]
    /// calls this.
    pub(crate) fn take_for_daemon(
        &self,
        reference: CredentialRef,
        host: &str,
    ) -> Result<Arc<SecretValue>, Denied> {
        let entries = self.read();
        let entry = entries.get(&reference).ok_or(Denied::Unknown)?;
        scope::check_daemon(&entry.credential, host).map_err(Denied::Refused)?;
        Ok(Arc::clone(&entry.value))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<CredentialRef, Entry>> {
        self.inner.entries.read().expect("vault lock")
    }

    /// Applies a change to a copy, writes it, and only then makes it current, so a failed write
    /// changes nothing.
    fn change(
        &self,
        apply: impl FnOnce(&mut BTreeMap<CredentialRef, Entry>) -> Result<(), VaultError>,
    ) -> Result<(), VaultError> {
        let mut entries = self.inner.entries.write().expect("vault lock");
        let mut next: BTreeMap<CredentialRef, Entry> = entries
            .iter()
            .map(|(r, e)| {
                (
                    *r,
                    Entry {
                        credential: e.credential.clone(),
                        value: Arc::clone(&e.value),
                    },
                )
            })
            .collect();
        apply(&mut next)?;
        persist(&self.inner.dir.join(CREDENTIALS), &self.inner.sealer, &next)?;
        self.inner.redaction.replace(patterns(&next));
        *entries = next;
        Ok(())
    }
}

fn patterns(entries: &BTreeMap<CredentialRef, Entry>) -> Patterns {
    Patterns::build(entries.iter().map(|(r, e)| (*r, e.value.as_ref())))
}

/// Each entry is the credential's record as JSON and then its value, each behind a four-byte
/// length. The buffer is sized up front so it is never reallocated, which would leave an unwiped
/// copy of the values behind.
fn persist(
    path: &Path,
    sealer: &Sealer,
    entries: &BTreeMap<CredentialRef, Entry>,
) -> Result<(), VaultError> {
    let records: Vec<Vec<u8>> = entries
        .values()
        .map(|e| serde_json::to_vec(&e.credential).expect("a credential serialises"))
        .collect();
    let size = records.iter().map(|r| 8 + r.len()).sum::<usize>()
        + entries.values().map(|e| e.value.len()).sum::<usize>();
    let mut plain = Zeroizing::new(Vec::with_capacity(size));
    for (record, entry) in records.iter().zip(entries.values()) {
        plain.extend_from_slice(&len32(record.len()));
        plain.extend_from_slice(record);
        plain.extend_from_slice(&len32(entry.value.len()));
        plain.extend_from_slice(entry.value.expose());
    }
    let sealed = sealer.seal(KIND, &plain)?;
    seal::write_atomic(path, &sealed, 0o600)
}

fn len32(n: usize) -> [u8; 4] {
    u32::try_from(n)
        .expect("a record under 4 GiB")
        .to_le_bytes()
}

fn decode(plain: &[u8], path: &Path) -> Result<BTreeMap<CredentialRef, Entry>, VaultError> {
    let corrupt = || {
        VaultError::Corrupt(format!(
            "{} opened but its contents are malformed",
            path.display()
        ))
    };
    let mut entries = BTreeMap::new();
    let mut rest = plain;
    let next = |rest: &mut &[u8]| -> Result<Vec<u8>, VaultError> {
        let (len, tail) = rest.split_first_chunk::<4>().ok_or_else(corrupt)?;
        let len = u32::from_le_bytes(*len) as usize;
        let field = tail.get(..len).ok_or_else(corrupt)?.to_vec();
        *rest = &tail[len..];
        Ok(field)
    };
    while !rest.is_empty() {
        let record = next(&mut rest)?;
        let credential: Credential = serde_json::from_slice(&record).map_err(|_| corrupt())?;
        let value = SecretValue::new(next(&mut rest)?);
        entries.insert(
            credential.reference,
            Entry {
                credential,
                value: Arc::new(value),
            },
        );
    }
    Ok(entries)
}
