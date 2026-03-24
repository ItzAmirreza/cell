use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use cell_format::ResourceLimits as FormatResourceLimits;
use cell_runtime::{self, ResourceLimits as RuntimeResourceLimits};
use cell_store::{BlobStore, ContainerStore, ImageStore};

use super::cell_home;

pub fn run(image: &str, command: Option<&str>) -> Result<()> {
    let home = cell_home();
    let image_store = ImageStore::new(home.join("images"))?;
    let blob_store = BlobStore::new(home.join("blobs"))?;
    let container_store = ContainerStore::with_root(home.clone())?;

    // Load the image manifest.
    // Try the literal name first, then fall back to a filesystem-safe form
    // (e.g. "alpine:3.19" -> "alpine_3.19") so that users can refer to
    // pulled images either way.
    let manifest = image_store.load(image).or_else(|_| {
        let safe = image.replace('/', "_").replace(':', "_");
        if safe != image {
            image_store.load(&safe)
        } else {
            Err(anyhow::anyhow!("image not found: {image}"))
        }
    }).with_context(|| format!("image not found: {image}"))?;

    println!("Creating container from '{}'...", manifest.name);

    // Create a container entry.
    let mut state = container_store.create(&manifest.name)?;
    // rootfs_path is already absolute (set by ContainerStore::create), so
    // use it directly instead of joining with home (which would double-prefix).
    let rootfs = state.rootfs_path.clone();
    fs::create_dir_all(&rootfs)?;

    // Extract layers into the rootfs.
    for layer_ref in &manifest.layers {
        let blob = blob_store.get(&layer_ref.digest)?;

        if layer_ref
            .media_type
            .contains("cell.layer")
        {
            // Cell-native JSON layer format.
            extract_cell_layer(&blob, &rootfs)
                .with_context(|| format!("failed to extract cell layer {}", layer_ref.digest))?;
        } else {
            // OCI gzip tar layer.
            extract_oci_layer(&blob, &rootfs)
                .with_context(|| format!("failed to extract OCI layer {}", layer_ref.digest))?;
        }
    }

    // Build environment variables from the image config.
    let env: Vec<(String, String)> = manifest
        .config
        .env
        .iter()
        .filter_map(|e| {
            let mut parts = e.splitn(2, '=');
            let key = parts.next()?.to_string();
            let value = parts.next().unwrap_or("").to_string();
            Some((key, value))
        })
        .collect();

    // Determine the command to run.
    let cmd = if let Some(c) = command {
        c.to_string()
    } else if let Some(ref ep) = manifest.config.entrypoint {
        ep.join(" ")
    } else {
        "/bin/sh".to_string()
    };

    // Load resource limits if stored during build.
    let limits = load_limits(&home, &manifest.name);

    // Create the guard (with or without resource limits).
    let guard = if let Some(limits) = limits {
        println!(
            "Applying resource limits: memory={}, processes={}",
            limits.memory.map_or("unlimited".to_string(), |m| format!("{} bytes", m)),
            limits.processes.map_or("unlimited".to_string(), |p| p.to_string()),
        );
        let rt_limits = RuntimeResourceLimits {
            memory_bytes: limits.memory.unwrap_or(0),
            max_processes: limits.processes.unwrap_or(0) as u32,
        };
        cell_runtime::create_guard_with_limits(rt_limits)
    } else {
        cell_runtime::create_guard()
    };

    println!("Running '{}' in container {}...", cmd, &state.id[..8]);

    // Run the process.
    let exit_code = guard.run(&mut state, &cmd, &env)?;

    // Persist final state.
    container_store.update(&state)?;

    println!("Container {} exited with code {}", &state.id[..8], exit_code);
    Ok(())
}

// ---------------------------------------------------------------------------
// Layer extraction
// ---------------------------------------------------------------------------

/// JSON structure used inside Cell-native layers (mirrors build.rs).
#[derive(Debug, Deserialize)]
struct LayerEntry {
    #[allow(dead_code)]
    dest: String,
    files: Vec<LayerFile>,
}

#[derive(Debug, Deserialize)]
struct LayerFile {
    path: String,
    data: String,
}

/// Extract a Cell-native JSON layer into the rootfs.
fn extract_cell_layer(blob: &[u8], rootfs: &Path) -> Result<()> {
    let entry: LayerEntry =
        serde_json::from_slice(blob).context("failed to parse cell layer JSON")?;

    for file in &entry.files {
        let decoded = super::build::base64_decode(&file.data)
            .context("failed to decode base64 layer data")?;

        // Strip leading '/' so the join works correctly.
        let rel = file.path.strip_prefix('/').unwrap_or(&file.path);
        let dest = rootfs.join(rel);

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, &decoded)
            .with_context(|| format!("failed to write {}", dest.display()))?;
    }

    Ok(())
}

/// Extract an OCI gzip-compressed tar layer into the rootfs.
///
/// Delegates to `cell_oci::pull::extract_layer` which handles gzip
/// decompression, whiteout markers, and best-effort unpacking.
fn extract_oci_layer(blob: &[u8], rootfs: &Path) -> Result<()> {
    cell_oci::pull::extract_layer(blob, rootfs)
}

/// Try to load resource limits that were saved during `cell build`.
fn load_limits(home: &Path, image_name: &str) -> Option<FormatResourceLimits> {
    let path = home.join("images").join(image_name).join("limits.json");
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}
