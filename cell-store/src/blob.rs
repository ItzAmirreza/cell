use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::hash::sha256_digest;

/// Content-addressed blob store backed by a flat directory.
///
/// Each blob is stored as a file whose name equals its digest (`sha256-{hex}`).
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Create a new `BlobStore`, ensuring the root directory exists.
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("failed to create blob store at {}", root.display()))?;
        Ok(Self { root })
    }

    /// Store `data` and return its digest.
    ///
    /// The write is atomic: data is first written to a temporary file, then
    /// renamed into place.  If a blob with the same digest already exists the
    /// write is skipped (content-addressed dedup).
    pub fn put(&self, data: &[u8]) -> Result<String> {
        let digest = sha256_digest(data);
        let dest = self.root.join(&digest);

        if dest.exists() {
            return Ok(digest);
        }

        let tmp = self.root.join(format!("{}.tmp", digest));
        fs::write(&tmp, data)
            .with_context(|| format!("failed to write temp blob {}", tmp.display()))?;
        fs::rename(&tmp, &dest)
            .with_context(|| format!("failed to rename blob into place {}", dest.display()))?;

        Ok(digest)
    }

    /// Read the blob identified by `digest`.
    pub fn get(&self, digest: &str) -> Result<Vec<u8>> {
        let path = self.root.join(digest);
        fs::read(&path).with_context(|| format!("blob not found: {digest}"))
    }

    /// Check whether a blob with the given digest exists.
    pub fn exists(&self, digest: &str) -> bool {
        self.root.join(digest).exists()
    }

    /// List the digests of every blob in the store.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut digests = Vec::new();
        for entry in fs::read_dir(&self.root).context("failed to list blob store")? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("sha256-") {
                digests.push(name.into_owned());
            }
        }
        digests.sort();
        Ok(digests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (BlobStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
        (store, tmp)
    }

    #[test]
    fn put_and_get() {
        let (s, _tmp) = store();
        let digest = s.put(b"hello").unwrap();
        assert!(digest.starts_with("sha256-"));
        assert_eq!(s.get(&digest).unwrap(), b"hello");
    }

    #[test]
    fn dedup() {
        let (s, _tmp) = store();
        let d1 = s.put(b"same").unwrap();
        let d2 = s.put(b"same").unwrap();
        assert_eq!(d1, d2);
    }

    #[test]
    fn exists_and_missing() {
        let (s, _tmp) = store();
        let d = s.put(b"data").unwrap();
        assert!(s.exists(&d));
        assert!(!s.exists("sha256-0000"));
    }

    #[test]
    fn list_blobs() {
        let (s, _tmp) = store();
        s.put(b"aaa").unwrap();
        s.put(b"bbb").unwrap();
        let digests = s.list().unwrap();
        assert_eq!(digests.len(), 2);
    }

    #[test]
    fn get_missing_returns_error() {
        let (s, _tmp) = store();
        assert!(s.get("sha256-nonexistent").is_err());
    }
}
