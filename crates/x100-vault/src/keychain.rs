//! Where the vault's own key lives, when the operating system offers a keychain.

use std::fmt;

use zeroize::Zeroizing;

/// The keychain cannot be reached at all, as opposed to having no entry for this vault.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the OS keychain is not available: {0}")]
pub struct Unavailable(pub String);

/// A store for the vault's key outside the vault's directory. [`OsKeychain`] is the real one;
/// [`NoKeychain`] makes the vault use a key file, which is what tests and machines without a
/// keychain use.
pub trait Keychain: Send + Sync {
    /// The secret stored for `account`, or `None` if there is none.
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, Unavailable>;
    fn set(&self, account: &str, secret: &[u8]) -> Result<(), Unavailable>;
}

/// No keychain: the vault keeps its key in a file only the builder's user can read.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoKeychain;

impl Keychain for NoKeychain {
    fn get(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, Unavailable> {
        Err(Unavailable("no keychain was asked for".into()))
    }

    fn set(&self, _: &str, _: &[u8]) -> Result<(), Unavailable> {
        Err(Unavailable("no keychain was asked for".into()))
    }
}

/// The service name entries are filed under.
const SERVICE: &str = "x100";

/// The operating system's keychain: the Secret Service over D-Bus on Linux, the login keychain on
/// macOS. Where it cannot be reached (Linux with no Secret Service, as under WSL), every call
/// answers [`Unavailable`] and the vault falls back to a key file.
pub struct OsKeychain {
    store: Result<std::sync::Arc<keyring_core::CredentialStore>, Unavailable>,
}

impl fmt::Debug for OsKeychain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.store {
            Ok(_) => "available",
            Err(_) => "unavailable",
        };
        f.debug_struct("OsKeychain").field("state", &state).finish()
    }
}

impl OsKeychain {
    /// Connects to the platform's keychain. This talks to D-Bus on Linux, so call it from a
    /// blocking context.
    pub fn connect() -> Self {
        Self {
            store: platform_store().map_err(|e| Unavailable(e.to_string())),
        }
    }

    fn entry(&self, account: &str) -> Result<keyring_core::Entry, Unavailable> {
        let store = self.store.as_ref().map_err(Clone::clone)?;
        store
            .build(SERVICE, account, None)
            .map_err(|e| Unavailable(e.to_string()))
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

impl Keychain for OsKeychain {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, Unavailable> {
        match self.entry(account)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(Unavailable(e.to_string())),
        }
    }

    fn set(&self, account: &str, secret: &[u8]) -> Result<(), Unavailable> {
        self.entry(account)?
            .set_secret(secret)
            .map_err(|e| Unavailable(e.to_string()))
    }
}
