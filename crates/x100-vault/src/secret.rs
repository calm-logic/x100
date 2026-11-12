//! A credential's value in memory.

use zeroize::Zeroizing;

/// A credential's value. It is wiped when dropped, and it has no `Debug`, `Display`, `Clone`,
/// `PartialEq` or serde implementation, so it cannot be printed, logged, serialised, copied or
/// compared by accident. Only this crate can read it back.
pub struct SecretValue(Zeroizing<Box<[u8]>>);

/// Shorter values cannot be told apart from ordinary output, so the redactor could neither find
/// them reliably nor avoid replacing text that only happens to match.
pub const MIN_LEN: usize = 8;

impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Self {
        // `into_boxed_slice` may reallocate, which would leave the old buffer unwiped.
        let bytes = Zeroizing::new(bytes);
        let mut boxed = vec![0u8; bytes.len()].into_boxed_slice();
        boxed.copy_from_slice(&bytes);
        Self(Zeroizing::new(boxed))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl From<String> for SecretValue {
    fn from(value: String) -> Self {
        Self::new(value.into_bytes())
    }
}

impl From<&str> for SecretValue {
    fn from(value: &str) -> Self {
        Self::new(value.as_bytes().to_vec())
    }
}
