use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::hash::sha256_digest;

/// Content-addressed blob storage.
///
/// Blobs are stored by their SHA-256 digest under a flat directory.
/// Duplicate writes are automatically deduplicated.
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Ensure the storage directory exists.
    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create blob store at {:?}", self.root))
    }

    /// Store `data` and return its digest. No-op if the blob already exists.
    pub fn put(&self, data: &[u8]) -> Result<String> {
        self.init()?;
        let digest = sha256_digest(data);
        let path = self.root.join(&digest);

        if path.exists() {
            return Ok(digest);
        }

        // Write to a temp file first, then rename for atomic write.
        let tmp_path = self.root.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
        fs::write(&tmp_path, data)
            .with_context(|| format!("failed to write temp blob {:?}", tmp_path))?;
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to rename blob to {:?}", path))?;

        Ok(digest)
    }

    /// Retrieve the contents of a blob by digest.
    pub fn get(&self, digest: &str) -> Result<Vec<u8>> {
        let path = self.root.join(digest);
        fs::read(&path).with_context(|| format!("blob not found: {digest}"))
    }

    /// Check if a blob exists.
    pub fn exists(&self, digest: &str) -> bool {
        self.root.join(digest).exists()
    }

    /// List all blob digests in the store.
    pub fn list(&self) -> Result<Vec<String>> {
        self.init()?;
        let mut digests = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("sha256-") {
                digests.push(name);
            }
        }
        digests.sort();
        Ok(digests)
    }

    /// Remove a blob by digest.
    pub fn remove(&self, digest: &str) -> Result<()> {
        let path = self.root.join(digest);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, BlobStore) {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = BlobStore::new(tmp.path().join("blobs"));
        (tmp, store)
    }

    #[test]
    fn test_put_and_get() {
        let (_tmp, store) = tmp_store();
        let data = b"hello world";
        let digest = store.put(data).unwrap();
        assert!(digest.starts_with("sha256-"));
        let retrieved = store.get(&digest).unwrap();
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_deduplication() {
        let (_tmp, store) = tmp_store();
        let d1 = store.put(b"same").unwrap();
        let d2 = store.put(b"same").unwrap();
        assert_eq!(d1, d2);
    }

    #[test]
    fn test_exists() {
        let (_tmp, store) = tmp_store();
        let digest = store.put(b"data").unwrap();
        assert!(store.exists(&digest));
        assert!(!store.exists("sha256-nonexistent"));
    }

    #[test]
    fn test_list() {
        let (_tmp, store) = tmp_store();
        store.put(b"aaa").unwrap();
        store.put(b"bbb").unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove() {
        let (_tmp, store) = tmp_store();
        let digest = store.put(b"removeme").unwrap();
        assert!(store.exists(&digest));
        store.remove(&digest).unwrap();
        assert!(!store.exists(&digest));
    }
}
