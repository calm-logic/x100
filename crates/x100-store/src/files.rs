//! Attachments and generated files, in a directory named by content: the same bytes stored twice
//! are one file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use x100_core::file::ContentHash;

use crate::error::{Error, Result, io};

/// A hash is written `sha256:` and 64 lowercase hex digits, which says what it is a hash of.
const PREFIX: &str = "sha256:";

#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(io(&dir))?;
        Ok(Self { dir })
    }

    /// Stores `bytes` and returns their hash. Storing what is already there changes nothing.
    pub fn put(&self, bytes: &[u8]) -> Result<ContentHash> {
        let hash = ContentHash(format!("{PREFIX}{:x}", Sha256::digest(bytes)));
        let path = self.path(&hash)?;
        if path.exists() {
            return Ok(hash);
        }
        let parent = path.parent().expect("a hashed path has a parent");
        fs::create_dir_all(parent).map_err(io(parent))?;
        // Written under another name and renamed, so a crash never leaves a half file under a
        // hash that claims to be whole.
        let part = parent.join(format!(".{}.part", &hash.0[PREFIX.len()..]));
        let mut f = fs::File::create(&part).map_err(io(&part))?;
        f.write_all(bytes).map_err(io(&part))?;
        f.sync_all().map_err(io(&part))?;
        fs::rename(&part, &path).map_err(io(&path))?;
        Ok(hash)
    }

    pub fn read(&self, hash: &ContentHash) -> Result<Vec<u8>> {
        let path = self.path(hash)?;
        fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NoSuchFile(hash.0.clone()),
            _ => Error::Io { path, source: e },
        })
    }

    pub fn contains(&self, hash: &ContentHash) -> Result<bool> {
        Ok(self.path(hash)?.exists())
    }

    /// Deletes the bytes. Whether anything still refers to them is the caller's to know. Returns
    /// whether there was a file.
    pub fn remove(&self, hash: &ContentHash) -> Result<bool> {
        let path = self.path(hash)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    /// Only a well-formed hash names a file, so a hash from outside cannot point out of the
    /// directory.
    fn path(&self, hash: &ContentHash) -> Result<PathBuf> {
        let h = hash
            .0
            .strip_prefix(PREFIX)
            .filter(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .ok_or_else(|| Error::BadHash(hash.0.clone()))?;
        Ok(self.dir.join(&h[..2]).join(&h[2..]))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
