use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Bytes that are not text: a file, or what a program wrote to its terminal. JSON carries them as
/// standard base64 text, so every value 0 to 255 survives.
#[derive(Clone, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD
            .decode(text)
            .map(Bytes)
            .map_err(serde::de::Error::custom)
    }
}
