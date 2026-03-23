use serde::{Deserialize, Serialize};

use crate::ast::{EnvVar, PortMapping, ResourceLimits, VolumeMount};

/// A reference to a content-addressed blob in the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentRef {
    pub digest: String,
    pub size: u64,
    pub media_type: String,
}

/// Configuration for a container image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageConfig {
    pub env: Vec<EnvVar>,
    pub entrypoint: Option<String>,
    pub exposed_ports: Vec<u16>,
    pub workdir: Option<String>,
}

/// A Cell image manifest — the root object stored per image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageManifest {
    pub name: String,
    pub created_at: String,
    pub config: ImageConfig,
    pub layers: Vec<ContentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ResourceLimits>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<VolumeMount>,
}
