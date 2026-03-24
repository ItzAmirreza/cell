use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use cell_format::ImageManifest;

/// Stores image manifests on disk, one directory per image name.
///
/// Layout:
/// ```text
/// <root>/
///   <name>/
///     manifest.json
/// ```
pub struct ImageStore {
    root: PathBuf,
}

impl ImageStore {
    /// Create a new `ImageStore`, ensuring the root directory exists.
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("failed to create image store at {}", root.display()))?;
        Ok(Self { root })
    }

    /// Persist an `ImageManifest`.  The file is written to
    /// `<root>/<manifest.name>/manifest.json`.
    pub fn save(&self, manifest: &ImageManifest) -> Result<()> {
        let dir = self.root.join(&manifest.name);
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create image dir {}", dir.display()))?;

        let path = dir.join("manifest.json");
        let json = serde_json::to_string_pretty(manifest).context("failed to serialize manifest")?;
        fs::write(&path, json)
            .with_context(|| format!("failed to write manifest {}", path.display()))?;

        Ok(())
    }

    /// Load an `ImageManifest` by image name.
    pub fn load(&self, name: &str) -> Result<ImageManifest> {
        let path = self.root.join(name).join("manifest.json");
        let data =
            fs::read_to_string(&path).with_context(|| format!("image not found: {name}"))?;
        let manifest: ImageManifest =
            serde_json::from_str(&data).context("failed to parse manifest")?;
        Ok(manifest)
    }

    /// List the names of all stored images.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.root).context("failed to list image store")? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let manifest_path = entry.path().join("manifest.json");
                if manifest_path.exists() {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Remove an image and its directory.
    pub fn remove(&self, name: &str) -> Result<()> {
        let dir = self.root.join(name);
        fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to remove image: {name}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cell_format::{ImageConfig, ImageManifest};
    use tempfile::TempDir;

    fn test_manifest(name: &str) -> ImageManifest {
        ImageManifest {
            name: name.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            config: ImageConfig {
                env: vec![],
                entrypoint: Some(vec!["/bin/sh".to_string()]),
                exposed_ports: vec![],
                workdir: None,
            },
            layers: vec![],
        }
    }

    fn store() -> (ImageStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = ImageStore::new(tmp.path().join("images")).unwrap();
        (store, tmp)
    }

    #[test]
    fn save_and_load() {
        let (s, _tmp) = store();
        let m = test_manifest("myimg");
        s.save(&m).unwrap();
        let loaded = s.load("myimg").unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn list_images() {
        let (s, _tmp) = store();
        s.save(&test_manifest("alpha")).unwrap();
        s.save(&test_manifest("beta")).unwrap();
        let names = s.list().unwrap();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn remove_image() {
        let (s, _tmp) = store();
        s.save(&test_manifest("gone")).unwrap();
        assert!(s.load("gone").is_ok());
        s.remove("gone").unwrap();
        assert!(s.load("gone").is_err());
    }

    #[test]
    fn load_missing_returns_error() {
        let (s, _tmp) = store();
        assert!(s.load("nope").is_err());
    }

    #[test]
    fn save_overwrites() {
        let (s, _tmp) = store();
        let mut m = test_manifest("img");
        s.save(&m).unwrap();

        m.created_at = "2026-06-01T00:00:00Z".to_string();
        s.save(&m).unwrap();

        let loaded = s.load("img").unwrap();
        assert_eq!(loaded.created_at, "2026-06-01T00:00:00Z");
    }
}
