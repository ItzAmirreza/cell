use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Tracks the lifecycle state of a container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContainerStatus {
    Created,
    Running,
    Stopped,
}

/// Persistent state for a single container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerState {
    pub id: String,
    pub image: String,
    pub status: ContainerStatus,
    pub pid: Option<u32>,
    pub created_at: String,
    pub rootfs_path: PathBuf,
}

/// Storage for container state files.
pub struct ContainerStore {
    root: PathBuf,
}

impl ContainerStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create container store at {:?}", self.root))
    }

    /// Create a new container record and return its state.
    pub fn create(&self, image: &str) -> Result<ContainerState> {
        self.init()?;
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let container_dir = self.root.join(&id);
        fs::create_dir_all(&container_dir)?;

        let rootfs_path = container_dir.join("rootfs");
        fs::create_dir_all(&rootfs_path)?;

        let state = ContainerState {
            id: id.clone(),
            image: image.to_string(),
            status: ContainerStatus::Created,
            pid: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            rootfs_path,
        };

        self.save(&state)?;
        Ok(state)
    }

    /// Save container state to disk.
    pub fn save(&self, state: &ContainerState) -> Result<()> {
        let path = self.root.join(&state.id).join("state.json");
        let json = serde_json::to_string_pretty(state)?;
        fs::write(&path, json)?;
        Ok(())
    }

    /// Load container state by ID (supports prefix matching).
    pub fn get(&self, id: &str) -> Result<ContainerState> {
        // Support prefix matching: "a1b2" matches "a1b2c3d4"
        let full_id = self.resolve_id(id)?;
        let path = self.root.join(&full_id).join("state.json");
        let json =
            fs::read_to_string(&path).with_context(|| format!("container not found: '{id}'"))?;
        Ok(serde_json::from_str(&json)?)
    }

    /// List all containers.
    pub fn list(&self) -> Result<Vec<ContainerState>> {
        self.init()?;
        let mut containers = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let state_path = entry.path().join("state.json");
            if state_path.exists() {
                let json = fs::read_to_string(&state_path)?;
                if let Ok(state) = serde_json::from_str::<ContainerState>(&json) {
                    containers.push(state);
                }
            }
        }
        containers.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(containers)
    }

    /// Remove a container directory entirely.
    pub fn remove(&self, id: &str) -> Result<()> {
        let full_id = self.resolve_id(id)?;
        let dir = self.root.join(&full_id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Resolve a possibly-abbreviated ID to a full container ID.
    fn resolve_id(&self, prefix: &str) -> Result<String> {
        if self.root.join(prefix).exists() {
            return Ok(prefix.to_string());
        }

        let mut matches = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(prefix) {
                    matches.push(name);
                }
            }
        }

        match matches.len() {
            0 => anyhow::bail!("no container found matching '{prefix}'"),
            1 => Ok(matches.into_iter().next().unwrap()),
            _ => anyhow::bail!("ambiguous container ID '{prefix}', matches: {matches:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_get() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ContainerStore::new(tmp.path().join("containers"));
        let state = store.create("myapp").unwrap();
        assert_eq!(state.image, "myapp");
        assert_eq!(state.status, ContainerStatus::Created);

        let loaded = store.get(&state.id).unwrap();
        assert_eq!(loaded.id, state.id);
    }

    #[test]
    fn test_list() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ContainerStore::new(tmp.path().join("containers"));
        store.create("app1").unwrap();
        store.create("app2").unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ContainerStore::new(tmp.path().join("containers"));
        let state = store.create("doomed").unwrap();
        store.remove(&state.id).unwrap();
        assert!(store.get(&state.id).is_err());
    }

    #[test]
    fn test_prefix_matching() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ContainerStore::new(tmp.path().join("containers"));
        let state = store.create("app").unwrap();
        // First 4 chars should match
        let prefix = &state.id[..4];
        let loaded = store.get(prefix).unwrap();
        assert_eq!(loaded.id, state.id);
    }
}
