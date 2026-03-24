use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle status of a container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    Created,
    Running,
    Stopped,
}

/// Persisted state for a single container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerState {
    pub id: String,
    pub image: String,
    pub status: ContainerStatus,
    pub created_at: String,
    pub pid: Option<u32>,
    pub rootfs_path: PathBuf,
}

/// On-disk store for container state.
///
/// Layout:
/// ```text
/// <root>/
///   containers/
///     <id>/
///       state.json
///       rootfs/
/// ```
pub struct ContainerStore {
    root: PathBuf,
}

impl ContainerStore {
    /// Return the default store root: `~/.cell`.
    fn default_root() -> PathBuf {
        dirs::home_dir()
            .expect("could not determine home directory")
            .join(".cell")
    }

    /// Create a `ContainerStore` at the default location (`~/.cell`).
    pub fn new() -> Result<Self> {
        Self::with_root(Self::default_root())
    }

    /// Create a `ContainerStore` rooted at `root`.
    pub fn with_root(root: PathBuf) -> Result<Self> {
        let containers = root.join("containers");
        fs::create_dir_all(&containers).with_context(|| {
            format!("failed to create container store at {}", containers.display())
        })?;
        Ok(Self { root })
    }

    fn containers_dir(&self) -> PathBuf {
        self.root.join("containers")
    }

    fn state_path(&self, id: &str) -> PathBuf {
        self.containers_dir().join(id).join("state.json")
    }

    /// Create a new container for the given image.
    ///
    /// Generates a short UUID identifier, creates the on-disk directory
    /// structure, writes the initial `state.json`, and returns the state.
    pub fn create(&self, image: &str) -> Result<ContainerState> {
        let id = Uuid::new_v4().to_string()[..12].to_string();

        let dir = self.containers_dir().join(&id);
        let rootfs = dir.join("rootfs");
        fs::create_dir_all(&rootfs)
            .with_context(|| format!("failed to create container dir {}", dir.display()))?;

        let state = ContainerState {
            id: id.clone(),
            image: image.to_string(),
            status: ContainerStatus::Created,
            created_at: Utc::now().to_rfc3339(),
            pid: None,
            rootfs_path: rootfs,
        };

        let json =
            serde_json::to_string_pretty(&state).context("failed to serialize container state")?;
        fs::write(self.state_path(&id), json).context("failed to write state.json")?;

        Ok(state)
    }

    /// Load container state by exact id or by unique prefix match.
    pub fn get(&self, id: &str) -> Result<ContainerState> {
        // Try exact match first.
        let exact = self.state_path(id);
        if exact.exists() {
            let data = fs::read_to_string(&exact).context("failed to read state.json")?;
            return serde_json::from_str(&data).context("failed to parse state.json");
        }

        // Prefix match.
        let mut matches: Vec<String> = Vec::new();
        for entry in fs::read_dir(self.containers_dir()).context("failed to list containers")? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(id) {
                matches.push(name);
            }
        }

        match matches.len() {
            0 => bail!("container not found: {id}"),
            1 => {
                let path = self.state_path(&matches[0]);
                let data = fs::read_to_string(&path).context("failed to read state.json")?;
                serde_json::from_str(&data).context("failed to parse state.json")
            }
            n => bail!("ambiguous container prefix '{id}': matches {n} containers"),
        }
    }

    /// Persist an updated `ContainerState`.
    pub fn update(&self, state: &ContainerState) -> Result<()> {
        let json =
            serde_json::to_string_pretty(state).context("failed to serialize container state")?;
        fs::write(self.state_path(&state.id), json).context("failed to write state.json")?;
        Ok(())
    }

    /// List every container in the store.
    pub fn list(&self) -> Result<Vec<ContainerState>> {
        let mut states = Vec::new();
        for entry in fs::read_dir(self.containers_dir()).context("failed to list containers")? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let path = entry.path().join("state.json");
                if path.exists() {
                    let data = fs::read_to_string(&path).context("failed to read state.json")?;
                    let state: ContainerState =
                        serde_json::from_str(&data).context("failed to parse state.json")?;
                    states.push(state);
                }
            }
        }
        states.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(states)
    }

    /// Remove a container and its entire directory tree.
    pub fn remove(&self, id: &str) -> Result<()> {
        // Resolve prefix first so `remove("abc")` works the same as `get("abc")`.
        let state = self.get(id)?;
        let dir = self.containers_dir().join(&state.id);
        fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to remove container {}", state.id))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (ContainerStore, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = ContainerStore::with_root(tmp.path().to_path_buf()).unwrap();
        (store, tmp)
    }

    #[test]
    fn create_container() {
        let (s, _tmp) = store();
        let c = s.create("myimage:latest").unwrap();
        assert_eq!(c.image, "myimage:latest");
        assert_eq!(c.status, ContainerStatus::Created);
        assert_eq!(c.id.len(), 12);
        assert!(c.pid.is_none());
        assert!(c.rootfs_path.ends_with(format!("{}/rootfs", c.id)));
    }

    #[test]
    fn get_by_exact_id() {
        let (s, _tmp) = store();
        let c = s.create("img").unwrap();
        let loaded = s.get(&c.id).unwrap();
        assert_eq!(loaded, c);
    }

    #[test]
    fn get_by_prefix() {
        let (s, _tmp) = store();
        let c = s.create("img").unwrap();
        // Use the first 4 characters as a prefix.
        let prefix = &c.id[..4];
        let loaded = s.get(prefix).unwrap();
        assert_eq!(loaded.id, c.id);
    }

    #[test]
    fn update_state() {
        let (s, _tmp) = store();
        let mut c = s.create("img").unwrap();
        c.status = ContainerStatus::Running;
        c.pid = Some(1234);
        s.update(&c).unwrap();

        let loaded = s.get(&c.id).unwrap();
        assert_eq!(loaded.status, ContainerStatus::Running);
        assert_eq!(loaded.pid, Some(1234));
    }

    #[test]
    fn list_containers() {
        let (s, _tmp) = store();
        s.create("a").unwrap();
        s.create("b").unwrap();
        let all = s.list().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn remove_container() {
        let (s, _tmp) = store();
        let c = s.create("img").unwrap();
        s.remove(&c.id).unwrap();
        assert!(s.get(&c.id).is_err());
    }

    #[test]
    fn get_missing_returns_error() {
        let (s, _tmp) = store();
        assert!(s.get("nonexistent").is_err());
    }
}
