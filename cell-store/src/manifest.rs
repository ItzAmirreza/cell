use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use cell_format::ImageManifest;

/// Storage for image manifests, organized by name.
pub struct ImageStore {
    root: PathBuf,
}

impl ImageStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Ensure the storage directory exists.
    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create image store at {:?}", self.root))
    }

    /// Save an image manifest. Overwrites if the image name already exists.
    pub fn save(&self, manifest: &ImageManifest) -> Result<()> {
        self.init()?;
        let dir = self.root.join(&manifest.name);
        fs::create_dir_all(&dir)?;
        let path = dir.join("manifest.json");
        let json = serde_json::to_string_pretty(manifest)?;
        fs::write(&path, json)
            .with_context(|| format!("failed to write manifest for '{}'", manifest.name))
    }

    /// Load an image manifest by name.
    pub fn load(&self, name: &str) -> Result<ImageManifest> {
        let path = self.root.join(name).join("manifest.json");
        let json =
            fs::read_to_string(&path).with_context(|| format!("image not found: '{name}'"))?;
        let manifest: ImageManifest = serde_json::from_str(&json)?;
        Ok(manifest)
    }

    /// List all image names in the store.
    pub fn list(&self) -> Result<Vec<String>> {
        self.init()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.path().is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Only include dirs that contain a manifest
                if entry.path().join("manifest.json").exists() {
                    names.push(name);
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Remove an image by name.
    pub fn remove(&self, name: &str) -> Result<()> {
        let dir = self.root.join(name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cell_format::{ImageConfig, ImageManifest};

    fn test_manifest(name: &str) -> ImageManifest {
        ImageManifest {
            name: name.to_string(),
            created_at: "2026-03-23T00:00:00Z".to_string(),
            config: ImageConfig {
                env: vec![],
                entrypoint: Some("/bin/sh".into()),
                exposed_ports: vec![],
                workdir: None,
            },
            layers: vec![],
            limits: None,
            ports: vec![],
            volumes: vec![],
        }
    }

    #[test]
    fn test_save_and_load() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ImageStore::new(tmp.path().join("images"));
        let manifest = test_manifest("myapp");
        store.save(&manifest).unwrap();
        let loaded = store.load("myapp").unwrap();
        assert_eq!(loaded.name, "myapp");
        assert_eq!(loaded.config.entrypoint.as_deref(), Some("/bin/sh"));
    }

    #[test]
    fn test_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ImageStore::new(tmp.path().join("images"));
        store.save(&test_manifest("alpha")).unwrap();
        store.save(&test_manifest("beta")).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_remove() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ImageStore::new(tmp.path().join("images"));
        store.save(&test_manifest("gone")).unwrap();
        store.remove("gone").unwrap();
        assert!(store.load("gone").is_err());
    }
}
