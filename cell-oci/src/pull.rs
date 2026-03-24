use std::path::Path;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;

use cell_format::{ContentRef, ImageConfig, ImageManifest};
use cell_store::{BlobStore, ImageStore};

use crate::convert::oci_config_to_cellspec;
use crate::registry::{ImageRef, OciConfig, RegistryClient};

/// Pull an OCI/Docker image from a remote registry and store it locally.
///
/// Returns the safe (filesystem-friendly) image name used as the store key
/// (e.g. `alpine_3.19` for `alpine:3.19`).
pub fn pull_image(reference: &str) -> Result<String> {
    let image = ImageRef::parse(reference)?;

    // Build a short, user-facing display name.  For Docker Hub library images
    // (repository = "library/foo") we strip the "library/" prefix so the user
    // can simply type `alpine:3.19` instead of `library/alpine:3.19`.
    let short_repo = image
        .repository
        .strip_prefix("library/")
        .unwrap_or(&image.repository);
    let display_name = format!("{}:{}", short_repo, image.tag);

    // The on-disk store key must be a single path component — replace
    // characters that are problematic in filenames.
    let image_name = display_name.replace('/', "_").replace(':', "_");

    eprintln!("Pulling {}...", image.full_ref());

    // --- authenticate ---
    let mut client = RegistryClient::new();
    client
        .authenticate(&image)
        .context("authentication failed")?;

    // --- fetch manifest (resolve fat manifests) ---
    eprintln!("Fetching manifest...");
    let manifest = client.get_manifest(&image)?;

    // --- fetch config ---
    eprintln!("Fetching config...");
    let oci_config: OciConfig = client.get_config(&image, &manifest)?;

    // --- set up local stores ---
    let cell_home = dirs::home_dir()
        .context("cannot determine home directory")?
        .join(".cell");

    let blob_store = BlobStore::new(cell_home.join("blobs"))?;
    let image_store = ImageStore::new(cell_home.join("images"))?;
    let layers_dir = cell_home.join("layers");
    std::fs::create_dir_all(&layers_dir)?;

    // --- download and store layers ---
    let mut layer_refs: Vec<ContentRef> = Vec::new();

    for (i, layer_desc) in manifest.layers.iter().enumerate() {
        let short_digest = &layer_desc.digest[..std::cmp::min(layer_desc.digest.len(), 19)];
        eprintln!(
            "Downloading layer {}/{}: {}...",
            i + 1,
            manifest.layers.len(),
            short_digest,
        );

        let data = client.get_blob(&image, &layer_desc.digest)?;
        let digest = blob_store.put(&data)?;

        // Extract layer into a directory named after its digest.
        let layer_dir = layers_dir.join(&digest);
        if !layer_dir.exists() {
            std::fs::create_dir_all(&layer_dir)?;
            extract_layer(&data, &layer_dir)
                .with_context(|| format!("failed to extract layer {}", short_digest))?;
        }

        layer_refs.push(ContentRef {
            digest,
            size: data.len() as u64,
            media_type: layer_desc.media_type.clone(),
        });
    }

    // --- convert OCI config to Cell manifest ---
    let container_config = oci_config.config.unwrap_or_default();
    let env = container_config.env.unwrap_or_default();
    let entrypoint = container_config.entrypoint.unwrap_or_default();
    let cmd = container_config.cmd.unwrap_or_default();
    let workdir = container_config.working_dir;

    let exposed_ports = parse_exposed_ports(&container_config.exposed_ports);

    let spec = oci_config_to_cellspec(
        &image_name,
        &env,
        &entrypoint,
        &cmd,
        &exposed_ports,
        workdir.as_deref(),
    );

    let created_at = chrono::Utc::now().to_rfc3339();

    let cell_manifest = ImageManifest {
        name: image_name.clone(),
        created_at,
        config: ImageConfig {
            env,
            entrypoint: if entrypoint.is_empty() {
                None
            } else {
                Some(entrypoint)
            },
            exposed_ports,
            workdir,
        },
        layers: layer_refs,
    };

    image_store.save(&cell_manifest)?;

    // Write a Cellfile alongside the manifest for convenience.
    let cellfile_text = crate::convert::cellspec_to_cellfile(&spec);
    let cellfile_path = cell_home.join("images").join(format!("{image_name}.Cellfile"));
    std::fs::write(&cellfile_path, cellfile_text)?;

    eprintln!("Image stored as '{}' ({})", image_name, display_name);
    Ok(image_name)
}

/// Extract a gzip-compressed tar layer to the given directory.
pub fn extract_layer(data: &[u8], target: &Path) -> Result<()> {
    let gz = GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(true);
    archive.set_overwrite(true);

    // Unpack entry by entry, skipping entries that fail (e.g. whiteout
    // markers, device nodes on non-root).
    for entry_result in archive.entries().context("failed to read tar entries")? {
        let mut entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        // Skip whiteout / opaque-whiteout markers.
        let path_bytes = entry.path_bytes();
        let path_lossy = String::from_utf8_lossy(&path_bytes);
        if path_lossy.contains(".wh.") {
            continue;
        }

        // Best-effort unpack — some entries (char devices, etc.) will fail
        // when not running as root; that is fine.
        let _ = entry.unpack_in(target);
    }

    Ok(())
}

/// Parse OCI `ExposedPorts` (a JSON object like `{"80/tcp":{}}`) into a
/// sorted `Vec<u16>`.
fn parse_exposed_ports(value: &Option<serde_json::Value>) -> Vec<u16> {
    let Some(serde_json::Value::Object(map)) = value else {
        return Vec::new();
    };

    let mut ports: Vec<u16> = map
        .keys()
        .filter_map(|k| {
            // Keys look like "80/tcp" or just "80".
            k.split('/').next().and_then(|p| p.parse().ok())
        })
        .collect();

    ports.sort();
    ports.dedup();
    ports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exposed_ports_empty() {
        assert!(parse_exposed_ports(&None).is_empty());
    }

    #[test]
    fn parse_exposed_ports_typical() {
        let val: serde_json::Value =
            serde_json::json!({"80/tcp": {}, "443/tcp": {}, "8080/tcp": {}});
        let ports = parse_exposed_ports(&Some(val));
        assert_eq!(ports, vec![80, 443, 8080]);
    }

    #[test]
    fn extract_layer_from_bytes() {
        // Build a tiny tar.gz in memory.
        let buf = Vec::new();
        let encoder = flate2::write::GzEncoder::new(buf, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let data = b"hello from layer";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "testfile.txt", &data[..])
            .unwrap();

        let compressed = builder.into_inner().unwrap().finish().unwrap();

        let tmp = std::env::temp_dir().join("cell_test_extract_layer");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        extract_layer(&compressed, &tmp).unwrap();

        let content = std::fs::read_to_string(tmp.join("testfile.txt")).unwrap();
        assert_eq!(content, "hello from layer");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
