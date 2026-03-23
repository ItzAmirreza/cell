use serde::{Deserialize, Serialize};

/// The top-level specification parsed from a Cellfile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellSpec {
    pub name: String,
    pub base: Option<String>,
    pub env: Vec<EnvVar>,
    pub fs_ops: Vec<FsOp>,
    pub run: Option<String>,
    pub expose: Vec<u16>,
    pub limits: Option<ResourceLimits>,
    /// Port mappings: host_port -> container_port.
    /// e.g., `ports { 8080 = 80 }` maps host:8080 to container:80.
    pub ports: Vec<PortMapping>,
    /// Named volumes: volume_name -> container_path.
    /// e.g., `volumes { "mydata" = "/app/data" }` mounts ~/.cell/volumes/mydata at /app/data.
    pub volumes: Vec<VolumeMount>,
}

/// Resource limits for the container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory: Option<String>,
    pub processes: Option<u32>,
}

impl ResourceLimits {
    pub fn memory_bytes(&self) -> Option<usize> {
        let s = self.memory.as_deref()?;
        let s = s.trim();
        let upper = s.to_uppercase();

        if upper.ends_with("GB") {
            let n: f64 = upper.trim_end_matches("GB").trim().parse().ok()?;
            Some((n * 1024.0 * 1024.0 * 1024.0) as usize)
        } else if upper.ends_with("MB") {
            let n: f64 = upper.trim_end_matches("MB").trim().parse().ok()?;
            Some((n * 1024.0 * 1024.0) as usize)
        } else if upper.ends_with("KB") {
            let n: f64 = upper.trim_end_matches("KB").trim().parse().ok()?;
            Some((n * 1024.0) as usize)
        } else {
            s.parse::<usize>().ok()
        }
    }
}

/// An environment variable key-value pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// A filesystem operation declared in the `fs` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FsOp {
    Copy { src: String, dest: String },
}

/// A port mapping: host_port <-> container_port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortMapping {
    pub host: u16,
    pub container: u16,
}

/// A named volume mount.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeMount {
    pub name: String,
    pub container_path: String,
}
