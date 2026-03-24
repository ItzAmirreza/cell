use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Image reference parsing
// ---------------------------------------------------------------------------

/// A parsed container image reference such as `registry-1.docker.io/library/nginx:latest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

impl ImageRef {
    /// Parse a human-friendly reference into its components.
    ///
    /// Handles the following forms:
    /// - `"nginx"` -> `registry-1.docker.io/library/nginx:latest`
    /// - `"nginx:1.25"` -> `registry-1.docker.io/library/nginx:1.25`
    /// - `"owner/repo"` -> `registry-1.docker.io/owner/repo:latest`
    /// - `"ghcr.io/owner/repo:v1"` -> `ghcr.io/owner/repo:v1`
    pub fn parse(reference: &str) -> Result<Self> {
        let reference = reference.trim();
        if reference.is_empty() {
            bail!("empty image reference");
        }

        // Split off the tag (last `:` that is not part of the registry port).
        let (name_part, tag) = split_tag(reference);

        // Decide whether there is an explicit registry.
        // A segment counts as a registry hostname if it contains a dot or a
        // colon (port) or equals "localhost".
        let parts: Vec<&str> = name_part.splitn(2, '/').collect();
        let (registry, repository) = if parts.len() == 1 {
            // Bare name like "nginx" — Docker Hub library image.
            (
                "registry-1.docker.io".to_string(),
                format!("library/{}", parts[0]),
            )
        } else {
            let first = parts[0];
            if looks_like_hostname(first) {
                (first.to_string(), parts[1].to_string())
            } else {
                // Two-part name without a dot, e.g. "owner/repo" on Docker Hub.
                (
                    "registry-1.docker.io".to_string(),
                    name_part.to_string(),
                )
            }
        };

        Ok(Self {
            registry,
            repository,
            tag,
        })
    }

    /// Canonical full reference string.
    pub fn full_ref(&self) -> String {
        format!("{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

/// Split a reference into the name portion and the tag, defaulting to "latest".
fn split_tag(reference: &str) -> (&str, String) {
    // We need to be careful not to split on a colon that belongs to a port
    // inside the registry hostname (e.g. "localhost:5000/repo:tag").
    // Strategy: find the last `/`, then look for `:` after it.
    if let Some(slash_pos) = reference.rfind('/') {
        let after_slash = &reference[slash_pos + 1..];
        if let Some(colon_pos) = after_slash.rfind(':') {
            let tag = &after_slash[colon_pos + 1..];
            let name = &reference[..slash_pos + 1 + colon_pos];
            (name, tag.to_string())
        } else {
            (reference, "latest".to_string())
        }
    } else {
        // No slash at all — simple name like "nginx" or "nginx:1.25".
        if let Some(colon_pos) = reference.rfind(':') {
            let name = &reference[..colon_pos];
            let tag = &reference[colon_pos + 1..];
            (name, tag.to_string())
        } else {
            (reference, "latest".to_string())
        }
    }
}

fn looks_like_hostname(segment: &str) -> bool {
    segment.contains('.') || segment.contains(':') || segment == "localhost"
}

// ---------------------------------------------------------------------------
// OCI / Docker distribution types
// ---------------------------------------------------------------------------

/// An OCI content descriptor (used in manifests and manifest lists).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciDescriptor {
    pub media_type: String,
    #[serde(default)]
    pub size: u64,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<OciPlatform>,
}

/// Platform specifier inside a manifest list entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciPlatform {
    pub architecture: String,
    pub os: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// An image manifest (OCI or Docker schema 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciManifest {
    #[serde(default)]
    pub schema_version: u32,
    pub media_type: Option<String>,
    pub config: OciDescriptor,
    pub layers: Vec<OciDescriptor>,
}

/// A manifest list / OCI index — selects manifests by platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OciPlatformManifest {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub manifests: Vec<OciDescriptor>,
}

/// OCI image configuration (the blob pointed to by the manifest `config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<OciContainerConfig>,
}

/// The container-runtime portion of the OCI config blob.
///
/// Docker / OCI capitalise these field names.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OciContainerConfig {
    #[serde(default, rename = "Env", skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<String>>,

    #[serde(
        default,
        rename = "Entrypoint",
        skip_serializing_if = "Option::is_none"
    )]
    pub entrypoint: Option<Vec<String>>,

    #[serde(default, rename = "Cmd", skip_serializing_if = "Option::is_none")]
    pub cmd: Option<Vec<String>>,

    #[serde(
        default,
        rename = "ExposedPorts",
        skip_serializing_if = "Option::is_none"
    )]
    pub exposed_ports: Option<serde_json::Value>,

    #[serde(
        default,
        rename = "WorkingDir",
        skip_serializing_if = "Option::is_none"
    )]
    pub working_dir: Option<String>,
}

// ---------------------------------------------------------------------------
// Registry client
// ---------------------------------------------------------------------------

/// Auth token response from Docker Hub.
#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}

/// Client for pulling manifests and blobs from an OCI distribution registry.
pub struct RegistryClient {
    client: reqwest::blocking::Client,
    token: Option<String>,
}

impl RegistryClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .user_agent("cell/0.1")
                .build()
                .expect("failed to build HTTP client"),
            token: None,
        }
    }

    /// Authenticate against the registry. For Docker Hub this performs the
    /// anonymous token dance via `auth.docker.io`.
    pub fn authenticate(&mut self, image: &ImageRef) -> Result<()> {
        if image.registry == "registry-1.docker.io" {
            let url = format!(
                "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{}:pull",
                image.repository
            );
            let resp: TokenResponse = self
                .client
                .get(&url)
                .send()
                .context("failed to request auth token")?
                .error_for_status()
                .context("auth token request failed")?
                .json()
                .context("failed to parse auth token")?;
            self.token = Some(resp.token);
        }
        // Other registries: anonymous access (no token needed for public repos).
        Ok(())
    }

    /// Fetch the raw manifest JSON, returning the bytes and the Content-Type.
    fn get_manifest_raw(&self, image: &ImageRef) -> Result<(Vec<u8>, String)> {
        let url = format!(
            "https://{}/v2/{}/manifests/{}",
            image.registry, image.repository, image.tag
        );

        let mut req = self.client.get(&url).header(
            "Accept",
            [
                "application/vnd.oci.image.index.v1+json",
                "application/vnd.oci.image.manifest.v1+json",
                "application/vnd.docker.distribution.manifest.v2+json",
                "application/vnd.docker.distribution.manifest.list.v2+json",
            ]
            .join(", "),
        );

        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .with_context(|| format!("failed to fetch manifest for {}", image.full_ref()))?
            .error_for_status()
            .with_context(|| format!("registry returned error for {}", image.full_ref()))?;

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let bytes = resp.bytes()?.to_vec();
        Ok((bytes, content_type))
    }

    /// Fetch and deserialise the image manifest. If the registry returns a
    /// manifest list (fat manifest), [`resolve_manifest`] is called to pick
    /// the `linux/amd64` variant.
    pub fn get_manifest(&self, image: &ImageRef) -> Result<OciManifest> {
        let (body, content_type) = self.get_manifest_raw(image)?;

        if content_type.contains("manifest.list") || content_type.contains("image.index") {
            let list: OciPlatformManifest = serde_json::from_slice(&body)
                .context("failed to parse manifest list")?;
            return self.resolve_manifest(image, &list);
        }

        // Try parsing as a manifest list first (some registries send it
        // without the expected content-type).
        if let Ok(list) = serde_json::from_slice::<OciPlatformManifest>(&body) {
            if !list.manifests.is_empty() {
                return self.resolve_manifest(image, &list);
            }
        }

        let manifest: OciManifest =
            serde_json::from_slice(&body).context("failed to parse image manifest")?;
        Ok(manifest)
    }

    /// Given a manifest list, find the `linux/amd64` entry and fetch its
    /// platform-specific manifest.
    pub fn resolve_manifest(
        &self,
        image: &ImageRef,
        list: &OciPlatformManifest,
    ) -> Result<OciManifest> {
        // Prefer linux/amd64.
        let entry = list
            .manifests
            .iter()
            .find(|d| {
                d.platform
                    .as_ref()
                    .map(|p| p.os == "linux" && p.architecture == "amd64")
                    .unwrap_or(false)
            })
            .or_else(|| list.manifests.first())
            .context("manifest list is empty")?;

        // Fetch the platform-specific manifest by digest.
        let platform_image = ImageRef {
            registry: image.registry.clone(),
            repository: image.repository.clone(),
            tag: entry.digest.clone(),
        };

        let (body, _ct) = self.get_manifest_raw(&platform_image)?;
        let manifest: OciManifest =
            serde_json::from_slice(&body).context("failed to parse platform manifest")?;
        Ok(manifest)
    }

    /// Download a blob by digest.
    pub fn get_blob(&self, image: &ImageRef, digest: &str) -> Result<Vec<u8>> {
        let url = format!(
            "https://{}/v2/{}/blobs/{}",
            image.registry, image.repository, digest
        );

        let mut req = self.client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let resp = req
            .send()
            .with_context(|| format!("failed to fetch blob {digest}"))?
            .error_for_status()
            .with_context(|| format!("registry error fetching blob {digest}"))?;

        Ok(resp.bytes()?.to_vec())
    }

    /// Download and parse the image config blob.
    pub fn get_config(&self, image: &ImageRef, manifest: &OciManifest) -> Result<OciConfig> {
        let data = self.get_blob(image, &manifest.config.digest)?;
        let config: OciConfig =
            serde_json::from_slice(&data).context("failed to parse OCI config")?;
        Ok(config)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_name() {
        let r = ImageRef::parse("nginx").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag, "latest");
    }

    #[test]
    fn parse_name_with_tag() {
        let r = ImageRef::parse("nginx:1.25").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag, "1.25");
    }

    #[test]
    fn parse_docker_hub_user_repo() {
        let r = ImageRef::parse("myuser/myrepo").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "myuser/myrepo");
        assert_eq!(r.tag, "latest");
    }

    #[test]
    fn parse_docker_hub_user_repo_tag() {
        let r = ImageRef::parse("myuser/myrepo:v2").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "myuser/myrepo");
        assert_eq!(r.tag, "v2");
    }

    #[test]
    fn parse_ghcr() {
        let r = ImageRef::parse("ghcr.io/owner/repo:v1").unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "owner/repo");
        assert_eq!(r.tag, "v1");
    }

    #[test]
    fn parse_ghcr_no_tag() {
        let r = ImageRef::parse("ghcr.io/owner/repo").unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "owner/repo");
        assert_eq!(r.tag, "latest");
    }

    #[test]
    fn parse_localhost_registry() {
        let r = ImageRef::parse("localhost:5000/myimage:dev").unwrap();
        assert_eq!(r.registry, "localhost:5000");
        assert_eq!(r.repository, "myimage");
        assert_eq!(r.tag, "dev");
    }

    #[test]
    fn parse_empty_is_error() {
        assert!(ImageRef::parse("").is_err());
    }

    #[test]
    fn full_ref_round_trip() {
        let r = ImageRef::parse("ghcr.io/owner/repo:v1").unwrap();
        assert_eq!(r.full_ref(), "ghcr.io/owner/repo:v1");
    }
}
