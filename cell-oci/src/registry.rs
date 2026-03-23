use anyhow::{Context, Result};
use serde::Deserialize;

/// Parsed image reference: `registry/name:tag`
#[derive(Debug, Clone)]
pub struct ImageRef {
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

impl ImageRef {
    /// Parse a Docker-style image reference.
    ///
    /// - `nginx` → `registry-1.docker.io/library/nginx:latest`
    /// - `nginx:1.25` → `registry-1.docker.io/library/nginx:1.25`
    /// - `ghcr.io/owner/repo:v1` → `ghcr.io/owner/repo:v1`
    pub fn parse(reference: &str) -> Self {
        let (name, tag) = match reference.rsplit_once(':') {
            Some((n, t)) if !n.contains('/') || !t.contains('/') => (n.to_string(), t.to_string()),
            _ => (reference.to_string(), "latest".to_string()),
        };

        if !name.contains('.') && !name.contains(':') {
            let repository = if name.contains('/') {
                name
            } else {
                format!("library/{name}")
            };
            ImageRef {
                registry: "registry-1.docker.io".into(),
                repository,
                tag,
            }
        } else {
            let (registry, repository) = name.split_once('/').unwrap_or(("", &name));
            ImageRef {
                registry: registry.to_string(),
                repository: repository.to_string(),
                tag,
            }
        }
    }

    pub fn full_ref(&self) -> String {
        format!("{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

/// Docker Registry HTTP API V2 client.
pub struct RegistryClient {
    client: reqwest::blocking::Client,
    token: Option<String>,
}

/// Docker Hub token response.
#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}

/// OCI/Docker image manifest (simplified).
#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OciManifest {
    pub schema_version: Option<u32>,
    pub media_type: Option<String>,
    pub config: Option<OciDescriptor>,
    pub layers: Option<Vec<OciDescriptor>>,
    /// For manifest list / fat manifests
    pub manifests: Option<Vec<OciPlatformManifest>>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OciDescriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
}

#[derive(Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OciPlatformManifest {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    pub platform: Option<OciPlatform>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct OciPlatform {
    pub architecture: String,
    pub os: String,
}

/// OCI image config (the "config blob").
#[derive(Debug, Deserialize)]
pub struct OciConfig {
    pub config: Option<OciContainerConfig>,
}

#[derive(Debug, Deserialize)]
pub struct OciContainerConfig {
    #[serde(rename = "Env")]
    pub env: Option<Vec<String>>,
    #[serde(rename = "Entrypoint")]
    pub entrypoint: Option<Vec<String>>,
    #[serde(rename = "Cmd")]
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "ExposedPorts")]
    pub exposed_ports: Option<serde_json::Value>,
    #[serde(rename = "WorkingDir")]
    pub working_dir: Option<String>,
}

impl RegistryClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::blocking::Client::builder()
                .user_agent("cell/0.1.0")
                .build()
                .expect("failed to create HTTP client"),
            token: None,
        }
    }

    /// Authenticate with Docker Hub for a given repository.
    pub fn authenticate(&mut self, image: &ImageRef) -> Result<()> {
        if image.registry != "registry-1.docker.io" {
            // Non-Docker Hub registries — skip auth for public repos
            return Ok(());
        }

        let url = format!(
            "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{}:pull",
            image.repository
        );

        let resp: TokenResponse = self
            .client
            .get(&url)
            .send()
            .context("failed to get auth token")?
            .json()
            .context("failed to parse auth token")?;

        self.token = Some(resp.token);
        Ok(())
    }

    /// Build the base URL for registry API calls.
    fn api_url(&self, image: &ImageRef) -> String {
        if image.registry == "registry-1.docker.io" {
            format!(
                "https://registry-1.docker.io/v2/{}",
                image.repository
            )
        } else {
            format!("https://{}/v2/{}", image.registry, image.repository)
        }
    }

    /// Add auth header to a request.
    fn auth_header(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(ref token) = self.token {
            req.bearer_auth(token)
        } else {
            req
        }
    }

    /// Fetch the image manifest.
    pub fn get_manifest(&self, image: &ImageRef) -> Result<OciManifest> {
        let url = format!("{}/manifests/{}", self.api_url(image), image.tag);

        let resp = self
            .auth_header(self.client.get(&url))
            .header(
                "Accept",
                "application/vnd.docker.distribution.manifest.v2+json, \
                 application/vnd.oci.image.manifest.v1+json, \
                 application/vnd.docker.distribution.manifest.list.v2+json, \
                 application/vnd.oci.image.index.v1+json",
            )
            .send()
            .context("failed to fetch manifest")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "manifest fetch failed: {} {}",
                resp.status(),
                resp.text().unwrap_or_default()
            );
        }

        let manifest: OciManifest = resp.json().context("failed to parse manifest")?;
        Ok(manifest)
    }

    /// Resolve a manifest list to a single-platform manifest.
    /// Prefers linux/amd64 since that's what most images have.
    pub fn resolve_manifest(&self, image: &ImageRef, manifest: &OciManifest) -> Result<OciManifest> {
        if let Some(ref manifests) = manifest.manifests {
            // This is a manifest list / fat manifest — pick the right platform
            let target_os = "linux";
            let target_arch = "amd64";

            let platform_manifest = manifests
                .iter()
                .find(|m| {
                    m.platform.as_ref().is_some_and(|p| {
                        p.os == target_os && p.architecture == target_arch
                    })
                })
                .or_else(|| manifests.first())
                .context("no suitable platform in manifest list")?;

            // Fetch the platform-specific manifest by digest
            let url = format!(
                "{}/manifests/{}",
                self.api_url(image),
                platform_manifest.digest
            );

            let resp = self
                .auth_header(self.client.get(&url))
                .header(
                    "Accept",
                    "application/vnd.docker.distribution.manifest.v2+json, \
                     application/vnd.oci.image.manifest.v1+json",
                )
                .send()
                .context("failed to fetch platform manifest")?;

            if !resp.status().is_success() {
                anyhow::bail!("platform manifest fetch failed: {}", resp.status());
            }

            Ok(resp.json().context("failed to parse platform manifest")?)
        } else {
            // Already a single-platform manifest
            Ok(manifest.clone())
        }
    }

    /// Download a blob (layer or config) by digest.
    pub fn get_blob(&self, image: &ImageRef, digest: &str) -> Result<Vec<u8>> {
        let url = format!("{}/blobs/{}", self.api_url(image), digest);

        let resp = self
            .auth_header(self.client.get(&url))
            .send()
            .context("failed to fetch blob")?;

        if !resp.status().is_success() {
            anyhow::bail!("blob fetch failed for {}: {}", digest, resp.status());
        }

        Ok(resp.bytes()?.to_vec())
    }

    /// Download the image config blob and parse it.
    pub fn get_config(&self, image: &ImageRef, manifest: &OciManifest) -> Result<OciConfig> {
        let config_desc = manifest
            .config
            .as_ref()
            .context("manifest has no config")?;

        let data = self.get_blob(image, &config_desc.digest)?;
        let config: OciConfig = serde_json::from_slice(&data)?;
        Ok(config)
    }
}

// Allow cloning the manifest for resolve_manifest
impl Clone for OciManifest {
    fn clone(&self) -> Self {
        // Quick and dirty clone via serde
        let json = serde_json::to_string(self).unwrap();
        serde_json::from_str(&json).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let r = ImageRef::parse("nginx");
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag, "latest");
    }

    #[test]
    fn test_parse_with_tag() {
        let r = ImageRef::parse("nginx:1.25");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag, "1.25");
    }

    #[test]
    fn test_parse_with_namespace() {
        let r = ImageRef::parse("myuser/myapp:v2");
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "myuser/myapp");
        assert_eq!(r.tag, "v2");
    }
}
