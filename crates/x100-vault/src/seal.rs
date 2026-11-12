//! The vault's key, and sealing files with it.
//!
//! A sealed file is one line of JSON header and then the ciphertext. The header says which key
//! sealed the file (the keychain's or the key file) and carries the nonce, and it is the
//! cipher's associated data, so changing it breaks the seal.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::VaultError;
use crate::keychain::Keychain;

pub(crate) const KEY_FILE: &str = "vault.key";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const FORMAT: u32 = 1;

/// Where the key that sealed a vault lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// The OS keychain, under the vault directory's path.
    Keychain,
    /// `vault.key` in the vault's directory, readable by the builder's user only.
    File,
}

#[derive(Serialize, Deserialize)]
struct Header {
    x100_vault: u32,
    /// What the file holds, so one sealed file cannot stand in for another.
    kind: String,
    key: KeySource,
    nonce: String,
}

pub(crate) struct Sealer {
    key: Zeroizing<[u8; KEY_LEN]>,
    source: KeySource,
}

impl Sealer {
    pub(crate) fn source(&self) -> KeySource {
        self.source
    }

    /// Finds the key for the vault in `dir`. A vault already sealed with one source must find
    /// its key there: a missing key is an error, never a fresh empty vault.
    pub(crate) fn unlock(
        dir: &Path,
        sealed: &Path,
        keychain: &dyn Keychain,
    ) -> Result<Self, VaultError> {
        let account = format!("vault-key {}", dir.display());
        let key_file = dir.join(KEY_FILE);
        match recorded_source(sealed)? {
            Some(KeySource::Keychain) => match keychain.get(&account) {
                Ok(Some(key)) => Self::from_bytes(&key, KeySource::Keychain),
                Ok(None) => Err(VaultError::KeyMissing(format!(
                    "{} was sealed with a key in the OS keychain, which has no entry `{account}`",
                    sealed.display()
                ))),
                Err(e) => Err(VaultError::KeychainUnavailable(e.0)),
            },
            Some(KeySource::File) => match read_key_file(&key_file)? {
                Some(key) => Self::from_bytes(&key, KeySource::File),
                None => Err(VaultError::KeyMissing(format!(
                    "{} was sealed with the key file {}, which is missing",
                    sealed.display(),
                    key_file.display()
                ))),
            },
            None => match keychain.get(&account) {
                Ok(Some(key)) => Self::from_bytes(&key, KeySource::Keychain),
                Ok(None) => {
                    let key = fresh_key()?;
                    match keychain.set(&account, key.as_slice()) {
                        Ok(()) => Ok(Self {
                            key,
                            source: KeySource::Keychain,
                        }),
                        Err(_) => key_file_sealer(&key_file),
                    }
                }
                Err(_) => key_file_sealer(&key_file),
            },
        }
    }

    fn from_bytes(bytes: &[u8], source: KeySource) -> Result<Self, VaultError> {
        let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| {
            VaultError::KeyMissing(format!(
                "the stored key is {} bytes, not {KEY_LEN}",
                bytes.len()
            ))
        })?;
        Ok(Self {
            key: Zeroizing::new(key),
            source,
        })
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(self.key.as_ref().into())
    }

    pub(crate) fn seal(&self, kind: &str, plain: &[u8]) -> Result<Vec<u8>, VaultError> {
        let mut nonce = [0u8; NONCE_LEN];
        random(&mut nonce)?;
        let header = Header {
            x100_vault: FORMAT,
            kind: kind.to_owned(),
            key: self.source,
            nonce: hex(&nonce),
        };
        let mut out = serde_json::to_vec(&header).expect("a header serialises");
        out.push(b'\n');
        let sealed = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: &out,
                },
            )
            .map_err(|_| VaultError::Corrupt("sealing failed".into()))?;
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    pub(crate) fn open(
        &self,
        kind: &str,
        path: &Path,
        bytes: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let (header, aad, sealed) = parse(path, bytes)?;
        if header.kind != kind {
            return Err(VaultError::Corrupt(format!(
                "{} holds `{}`, not `{kind}`",
                path.display(),
                header.kind
            )));
        }
        let nonce = unhex(&header.nonce)
            .filter(|n| n.len() == NONCE_LEN)
            .ok_or_else(|| VaultError::Corrupt(format!("{} has a bad nonce", path.display())))?;
        self.cipher()
            .decrypt(XNonce::from_slice(&nonce), Payload { msg: sealed, aad })
            .map(Zeroizing::new)
            .map_err(|_| VaultError::WrongKey(path.to_owned()))
    }
}

fn parse<'a>(path: &Path, bytes: &'a [u8]) -> Result<(Header, &'a [u8], &'a [u8]), VaultError> {
    let corrupt = || VaultError::Corrupt(format!("{} is not a sealed x100 file", path.display()));
    let end = memchr::memchr(b'\n', bytes).ok_or_else(corrupt)?;
    let header: Header = serde_json::from_slice(&bytes[..end]).map_err(|_| corrupt())?;
    if header.x100_vault != FORMAT {
        return Err(VaultError::Corrupt(format!(
            "{} has format {}, this build reads {FORMAT}",
            path.display(),
            header.x100_vault
        )));
    }
    Ok((header, &bytes[..=end], &bytes[end + 1..]))
}

fn recorded_source(sealed: &Path) -> Result<Option<KeySource>, VaultError> {
    match fs::read(sealed) {
        Ok(bytes) => Ok(Some(parse(sealed, &bytes)?.0.key)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(VaultError::Io(sealed.to_owned(), e)),
    }
}

fn key_file_sealer(path: &Path) -> Result<Sealer, VaultError> {
    if let Some(key) = read_key_file(path)? {
        return Sealer::from_bytes(&key, KeySource::File);
    }
    let key = fresh_key()?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| VaultError::Io(path.to_owned(), e))?;
    file.write_all(key.as_slice())
        .and_then(|()| file.sync_all())
        .map_err(|e| VaultError::Io(path.to_owned(), e))?;
    Ok(Sealer {
        key,
        source: KeySource::File,
    })
}

/// The key file's bytes, refusing a file anyone but its owner can read, as ssh does.
fn read_key_file(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(VaultError::Io(path.to_owned(), e)),
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(VaultError::KeyFileExposed(path.to_owned(), mode));
    }
    fs::read(path)
        .map(|b| Some(Zeroizing::new(b)))
        .map_err(|e| VaultError::Io(path.to_owned(), e))
}

fn fresh_key() -> Result<Zeroizing<[u8; KEY_LEN]>, VaultError> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    random(key.as_mut())?;
    Ok(key)
}

pub(crate) fn random(buf: &mut [u8]) -> Result<(), VaultError> {
    getrandom::fill(buf).map_err(|e| VaultError::Corrupt(format!("no randomness: {e}")))
}

/// Creates the vault's directory for the builder's user only, if it does not exist.
pub(crate) fn ensure_dir(dir: &Path) -> Result<(), VaultError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| VaultError::Io(dir.to_owned(), e))
}

/// Replaces `path` whole, so a crash leaves the old file or the new one, never half of each.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), VaultError> {
    let tmp: PathBuf = path.with_extension("tmp");
    let io = |e| VaultError::Io(path.to_owned(), e);
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&tmp)
        .map_err(io)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io)?;
    fs::rename(&tmp, path).map_err(io)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}
