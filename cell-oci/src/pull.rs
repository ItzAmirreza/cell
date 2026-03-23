use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{Context, Result};
use cell_format::{ContentRef, EnvVar, ImageConfig, ImageManifest};
use cell_store::{BlobStore, ImageStore};
use flate2::read::GzDecoder;

use crate::registry::{ImageRef, RegistryClient};

/// Get the Cell home directory.
fn cell_home() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".cell")
}

/// Pull an OCI image from a registry and convert it to Cell format.
pub fn pull_image(reference: &str) -> Result<String> {
    let image = ImageRef::parse(reference);
    let home = cell_home();
    let blobs = BlobStore::new(home.join("store").join("blobs"));
    let images = ImageStore::new(home.join("store").join("images"));

    println!("  resolving {}...", image.full_ref());

    // 1. Authenticate
    let mut client = RegistryClient::new();
    client.authenticate(&image)?;

    // 2. Get manifest (may be a manifest list)
    let raw_manifest = client.get_manifest(&image)?;

    // 3. Resolve to a single-platform manifest
    let manifest = client.resolve_manifest(&image, &raw_manifest)?;

    // 4. Get the image config
    let config = client.get_config(&image, &manifest)?;

    // 5. Download and store each layer
    let layer_descs = manifest.layers.as_ref().context("manifest has no layers")?;
    let mut cell_layers = Vec::new();

    for (i, layer) in layer_descs.iter().enumerate() {
        let short_digest = if layer.digest.len() > 19 {
            &layer.digest[..19]
        } else {
            &layer.digest
        };
        let size_mb = layer.size as f64 / 1024.0 / 1024.0;
        println!(
            "  layer {}/{}: {} ({:.1} MB)",
            i + 1,
            layer_descs.len(),
            short_digest,
            size_mb
        );

        // Check if we already have this blob (dedup)
        let cell_digest_check = format!("sha256-{}", &layer.digest[7..]); // convert "sha256:abc" to "sha256-abc"
        if blobs.exists(&cell_digest_check) {
            println!("    already exists, skipping download");
            cell_layers.push(ContentRef {
                digest: cell_digest_check,
                size: layer.size,
                media_type: "application/cell.layer.v1".into(),
            });
            continue;
        }

        // Download the layer blob
        let data = client.get_blob(&image, &layer.digest)?;

        // Store the raw compressed layer
        let digest = blobs.put(&data)?;
        println!("    stored as {}", &digest[..20]);

        cell_layers.push(ContentRef {
            digest,
            size: data.len() as u64,
            media_type: "application/cell.layer.v1".into(),
        });
    }

    // 6. Build the Cell image manifest
    let container_config = config.config.as_ref();

    let env_vars = container_config
        .and_then(|c| c.env.as_ref())
        .map(|envs| {
            envs.iter()
                .filter_map(|e| {
                    let (k, v) = e.split_once('=')?;
                    Some(EnvVar {
                        key: k.to_string(),
                        value: v.to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let entrypoint = container_config.and_then(|c| {
        c.entrypoint
            .as_ref()
            .map(|ep| ep.join(" "))
            .or_else(|| c.cmd.as_ref().map(|cmd| cmd.join(" ")))
    });

    let exposed_ports = container_config
        .and_then(|c| c.exposed_ports.as_ref())
        .and_then(|ports| {
            if let serde_json::Value::Object(map) = ports {
                Some(
                    map.keys()
                        .filter_map(|k| k.split('/').next()?.parse::<u16>().ok())
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Use a safe name for the image (replace / and : with _)
    let safe_name = reference
        .replace('/', "_")
        .replace(':', "_");

    let cell_manifest = ImageManifest {
        name: safe_name.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        config: ImageConfig {
            env: env_vars,
            entrypoint,
            exposed_ports,
            workdir: container_config.and_then(|c| c.working_dir.clone()),
        },
        layers: cell_layers,
        limits: None,
        ports: vec![],
        volumes: vec![],
    };

    images.save(&cell_manifest)?;

    println!("  image '{}' saved ({} layers)", safe_name, cell_manifest.layers.len());

    Ok(safe_name)
}

/// Extract a gzipped tar layer into a target directory.
/// Windows-safe: skips symlinks, hardlinks, device nodes, and whiteout files.
/// Handles path separators and permission errors gracefully.
pub fn extract_layer(data: &[u8], target: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(target)?;

    let decoder = GzDecoder::new(Cursor::new(data));
    let mut archive = tar::Archive::new(decoder);
    archive.set_preserve_permissions(false);
    archive.set_preserve_mtime(false);
    // Don't overwrite — later layers should override earlier ones
    archive.set_overwrite(true);

    let mut extracted = 0u32;
    let mut skipped = 0u32;

    for entry_result in archive.entries().context("failed to read tar entries")? {
        let mut entry = match entry_result {
            Ok(e) => e,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        let entry_type = entry.header().entry_type();

        // Skip symlinks, hardlinks, device nodes, FIFOs — Windows can't handle these
        match entry_type {
            tar::EntryType::Symlink
            | tar::EntryType::Link
            | tar::EntryType::Char
            | tar::EntryType::Block
            | tar::EntryType::Fifo => {
                skipped += 1;
                continue;
            }
            _ => {}
        }

        let path = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        // Skip OCI whiteout files (.wh.*)
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name.starts_with(".wh.") {
            skipped += 1;
            continue;
        }

        // Skip paths that try to escape the target (path traversal)
        let full_path = target.join(&path);
        if !full_path.starts_with(target) {
            skipped += 1;
            continue;
        }

        match entry_type {
            tar::EntryType::Directory => {
                let _ = std::fs::create_dir_all(&full_path);
                extracted += 1;
            }
            tar::EntryType::Regular | tar::EntryType::GNUSparse => {
                if let Some(parent) = full_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Read entry data and write to file
                let mut data = Vec::new();
                if std::io::Read::read_to_end(&mut entry, &mut data).is_ok() {
                    if std::fs::write(&full_path, &data).is_ok() {
                        extracted += 1;
                    } else {
                        skipped += 1;
                    }
                } else {
                    skipped += 1;
                }
            }
            _ => {
                skipped += 1;
            }
        }
    }

    if extracted == 0 && skipped > 0 {
        anyhow::bail!("no files extracted ({skipped} skipped)");
    }

    Ok(())
}
