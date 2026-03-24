use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use cell_format::{ContentRef, FsOp, ImageConfig, ImageManifest, Parser};
use cell_oci::registry::ImageRef;
use cell_store::{BlobStore, ImageStore};

use super::cell_home;

// ---------------------------------------------------------------------------
// Layer JSON format — content-addressed blobs store files in this structure
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct LayerEntry {
    dest: String,
    files: Vec<LayerFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LayerFile {
    path: String,
    data: String, // base64-encoded file contents
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

pub fn build(path: &str) -> Result<()> {
    let cellfile_path = resolve_cellfile(path)?;
    let cellfile_dir = cellfile_path
        .parent()
        .context("Cellfile has no parent directory")?;

    let source = fs::read_to_string(&cellfile_path)
        .with_context(|| format!("failed to read {}", cellfile_path.display()))?;

    let spec = Parser::parse(&source)
        .map_err(|e| anyhow::anyhow!("parse error: {e}"))?;

    println!("Building image '{}'...", spec.name);

    let home = cell_home();
    let blob_store = BlobStore::new(home.join("blobs"))?;
    let image_store = ImageStore::new(home.join("images"))?;

    // --- Base image auto-pull -------------------------------------------
    // If `base` is set to something other than "scratch", pull the base
    // image (unless it already exists locally) and collect its layers and
    // config so they can be merged into the final manifest.
    let base_manifest: Option<ImageManifest> = if spec.base != "scratch" {
        // Compute the filesystem-safe store name for the base reference.
        // This must match the naming scheme used by `pull_image()`.
        let base_ref = ImageRef::parse(&spec.base)
            .with_context(|| format!("invalid base image reference: {}", spec.base))?;
        let short_repo = base_ref
            .repository
            .strip_prefix("library/")
            .unwrap_or(&base_ref.repository);
        let canonical_name = format!("{}:{}", short_repo, base_ref.tag)
            .replace('/', "_")
            .replace(':', "_");

        // Check if it already exists locally.
        match image_store.load(&canonical_name) {
            Ok(manifest) => {
                println!("Base image '{}' found locally.", canonical_name);
                Some(manifest)
            }
            Err(_) => {
                println!("Pulling base image '{}'...", spec.base);
                let pulled_name = cell_oci::pull::pull_image(&spec.base)
                    .with_context(|| format!("failed to pull base image: {}", spec.base))?;
                let manifest = image_store.load(&pulled_name)
                    .with_context(|| format!("base image '{}' not found after pull", pulled_name))?;
                Some(manifest)
            }
        }
    } else {
        None
    };

    // Process fs operations into content-addressed layers.
    let mut layers: Vec<ContentRef> = Vec::new();

    for op in &spec.fs_ops {
        match op {
            FsOp::Copy { src, dest } => {
                let src_path = cellfile_dir.join(src);
                let entry = if src_path.is_dir() {
                    build_dir_layer(&src_path, dest)
                        .with_context(|| format!("failed to copy directory {}", src_path.display()))?
                } else if src_path.is_file() {
                    build_file_layer(&src_path, dest)
                        .with_context(|| format!("failed to copy file {}", src_path.display()))?
                } else {
                    anyhow::bail!(
                        "source path does not exist: {}",
                        src_path.display()
                    );
                };

                let json = serde_json::to_vec(&entry)
                    .context("failed to serialize layer")?;
                let digest = blob_store.put(&json)?;
                let size = json.len() as u64;

                println!(
                    "  layer: {} -> {} ({} file(s), {} bytes)",
                    src,
                    dest,
                    entry.files.len(),
                    size,
                );

                layers.push(ContentRef {
                    digest,
                    size,
                    media_type: "application/vnd.cell.layer.v1+json".to_string(),
                });
            }
        }
    }

    // Build env list for the image config.
    let container_env: Vec<String> = spec
        .env
        .iter()
        .map(|e| format!("{}={}", e.key, e.value))
        .collect();

    let container_entrypoint = spec.run.as_ref().map(|r| {
        vec!["/bin/sh".to_string(), "-c".to_string(), r.clone()]
    });

    // --- Merge base image layers and config -----------------------------
    // Base layers go first (underneath), container layers on top.
    let mut final_layers: Vec<ContentRef> = Vec::new();
    let mut merged_env = Vec::new();
    let mut merged_entrypoint: Option<Vec<String>> = None;
    let mut merged_workdir: Option<String> = None;

    if let Some(ref base) = base_manifest {
        // Prepend the base image's layers.
        final_layers.extend(base.layers.clone());

        // Start with the base image's env vars.
        merged_env.extend(base.config.env.clone());

        // Use the base image's entrypoint as a fallback.
        merged_entrypoint = base.config.entrypoint.clone();

        // Use the base image's workdir as a fallback.
        merged_workdir = base.config.workdir.clone();
    }

    // Append the container's own layers on top.
    final_layers.extend(layers);

    // Merge env: base env first, then overlay container env.  If the
    // container defines a variable that already exists in the base, the
    // container's value wins.
    for cv in &container_env {
        let key = cv.splitn(2, '=').next().unwrap_or("");
        // Remove any base entry with the same key.
        merged_env.retain(|existing: &String| {
            existing.splitn(2, '=').next().unwrap_or("") != key
        });
        merged_env.push(cv.clone());
    }

    // Container entrypoint takes precedence over base.
    if container_entrypoint.is_some() {
        merged_entrypoint = container_entrypoint;
    }

    let config = ImageConfig {
        env: merged_env,
        entrypoint: merged_entrypoint,
        exposed_ports: spec.expose.clone(),
        workdir: merged_workdir,
    };

    let manifest = ImageManifest {
        name: spec.name.clone(),
        created_at: Utc::now().to_rfc3339(),
        config,
        layers: final_layers,
    };

    image_store.save(&manifest)?;

    // If resource limits were specified, store them alongside the manifest
    // so that `cell run` can apply them.
    if let Some(limits) = &spec.limits {
        let limits_path = home
            .join("images")
            .join(&spec.name)
            .join("limits.json");
        let json = serde_json::to_string_pretty(limits)
            .context("failed to serialize resource limits")?;
        fs::write(&limits_path, json)
            .context("failed to write limits.json")?;
    }

    println!("Image '{}' built successfully.", spec.name);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the Cellfile path: if `path` is a directory, look for `Cellfile`
/// inside it; otherwise use it directly.
fn resolve_cellfile(path: &str) -> Result<std::path::PathBuf> {
    let p = Path::new(path);
    if p.is_dir() {
        let candidate = p.join("Cellfile");
        if candidate.exists() {
            return Ok(candidate);
        }
        anyhow::bail!("no Cellfile found in directory: {}", p.display());
    }
    if p.exists() {
        Ok(p.to_path_buf())
    } else {
        anyhow::bail!("path does not exist: {}", p.display());
    }
}

/// Build a [`LayerEntry`] from a single file copy.
fn build_file_layer(src: &Path, dest: &str) -> Result<LayerEntry> {
    let data = fs::read(src)
        .with_context(|| format!("failed to read {}", src.display()))?;

    let file_name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    // Determine the full destination path. If dest ends with '/' it is a
    // directory — append the filename.
    let full_dest = if dest.ends_with('/') {
        format!("{}{}", dest, file_name)
    } else {
        dest.to_string()
    };

    Ok(LayerEntry {
        dest: dest.to_string(),
        files: vec![LayerFile {
            path: full_dest,
            data: base64_encode(&data),
        }],
    })
}

/// Build a [`LayerEntry`] from a directory copy (recursive).
fn build_dir_layer(src: &Path, dest: &str) -> Result<LayerEntry> {
    let mut files = Vec::new();
    collect_files(src, src, dest, &mut files)?;
    Ok(LayerEntry {
        dest: dest.to_string(),
        files,
    })
}

fn collect_files(
    base: &Path,
    current: &Path,
    dest: &str,
    files: &mut Vec<LayerFile>,
) -> Result<()> {
    for entry in fs::read_dir(current)
        .with_context(|| format!("failed to read directory {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, dest, files)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(base)
                .context("failed to compute relative path")?;
            let dest_path = format!(
                "{}/{}",
                dest.trim_end_matches('/'),
                relative.to_string_lossy()
            );
            let data = fs::read(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            files.push(LayerFile {
                path: dest_path,
                data: base64_encode(&data),
            });
        }
    }
    Ok(())
}

/// Simple base64 encoder (avoids pulling in a full base64 crate).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Simple base64 decoder (mirror of `base64_encode`).
pub fn base64_decode(encoded: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Result<u32> {
        match c {
            b'A'..=b'Z' => Ok((c - b'A') as u32),
            b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
            b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => anyhow::bail!("invalid base64 character: {}", c as char),
        }
    }

    let input: Vec<u8> = encoded.bytes().filter(|b| *b != b'\n' && *b != b'\r').collect();
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    for chunk in input.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        out.push(((a << 2) | (b >> 4)) as u8);

        if chunk.len() > 2 && chunk[2] != b'=' {
            let c = val(chunk[2])?;
            out.push((((b & 0xF) << 4) | (c >> 2)) as u8);

            if chunk.len() > 3 && chunk[3] != b'=' {
                let d = val(chunk[3])?;
                out.push((((c & 0x3) << 6) | d) as u8);
            }
        }
    }
    Ok(out)
}
