use serde::{Deserialize, Serialize};

/// A content-addressable reference to a blob (layer, config, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRef {
    /// Digest string, e.g. "sha256:abc123...".
    pub digest: String,
    /// Size in bytes.
    pub size: u64,
    /// Media type, e.g. "application/vnd.oci.image.layer.v1.tar+gzip".
    pub media_type: String,
}

/// Runtime configuration carried inside an image manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposed_ports: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

/// An image manifest produced after building a cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageManifest {
    pub name: String,
    pub created_at: String,
    pub config: ImageConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<ContentRef>,
}
