use std::path::Path;

use anyhow::{Context, Result};
use cell_store::{BlobStore, ContainerStore, ImageStore};
use colored::Colorize;

use super::cell_home;

/// A layer entry from cell build.
#[derive(serde::Deserialize)]
struct LayerEntry {
    dest: String,
    files: Vec<LayerFile>,
}

#[derive(serde::Deserialize)]
struct LayerFile {
    path: String,
    data: Vec<u8>,
}

pub fn execute(image: &str, command: Option<&str>, interactive: bool) -> Result<()> {
    let home = cell_home();
    let blobs = BlobStore::new(home.join("store").join("blobs"));
    let images = ImageStore::new(home.join("store").join("images"));
    let containers = ContainerStore::new(home.join("containers"));

    let manifest = images
        .load(image)
        .with_context(|| format!("image '{image}' not found. Use 'cell build' or 'cell pull'."))?;

    let cmd = command
        .map(|s| s.to_string())
        .or(manifest.config.entrypoint.clone())
        .unwrap_or_else(|| "cmd.exe".to_string());

    let mut state = containers.create(image)?;
    let json_mode = super::is_json();
    if json_mode {
        eprintln!("Container {} created from image '{}'", state.id, image);
    } else {
        println!("{} {} from image '{}'", "Container".cyan(), state.id.bold(), image.bold());
    }

    // Extract layers into rootfs
    if !manifest.layers.is_empty() {
        if json_mode {
            eprintln!("Preparing rootfs ({} layers)...", manifest.layers.len());
        } else {
            println!("Preparing rootfs ({} layers)...", manifest.layers.len());
        }
        for (i, layer) in manifest.layers.iter().enumerate() {
            if let Ok(data) = blobs.get(&layer.digest) {
                let short = if layer.digest.len() > 16 {
                    &layer.digest[..16]
                } else {
                    &layer.digest
                };

                let media = &layer.media_type;
                if media.contains("+json") || media == "application/cell.layer.v1+json" {
                    // Cell-native layer format (from cell build)
                    match extract_cell_layer(&data, &state.rootfs_path) {
                        Ok(n) => {
                            if json_mode {
                                eprintln!("  layer {}/{} ({}) -> {} file(s)", i + 1, manifest.layers.len(), short, n);
                            } else {
                                println!("  layer {}/{} ({}) -> {} file(s)", i + 1, manifest.layers.len(), short, n);
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                eprintln!("  layer {}/{} ({}) skip: {}", i + 1, manifest.layers.len(), short, e);
                            } else {
                                println!("  layer {}/{} ({}) skip: {}", i + 1, manifest.layers.len(), short, e);
                            }
                        }
                    }
                } else {
                    // OCI layer (gzip tar from cell pull)
                    match cell_oci::pull::extract_layer(&data, &state.rootfs_path) {
                        Ok(_) => {
                            if json_mode {
                                eprintln!("  layer {}/{} ({}) ok (oci)", i + 1, manifest.layers.len(), short);
                            } else {
                                println!("  layer {}/{} ({}) ok (oci)", i + 1, manifest.layers.len(), short);
                            }
                        }
                        Err(e) => {
                            if json_mode {
                                eprintln!("  layer {}/{} ({}) skip: {}", i + 1, manifest.layers.len(), short, e);
                            } else {
                                println!("  layer {}/{} ({}) skip: {}", i + 1, manifest.layers.len(), short, e);
                            }
                        }
                    }
                }
            }
        }
    }

    let env: Vec<(String, String)> = manifest
        .config
        .env
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect();

    let guard = cell_runtime::create_guard(&manifest);
    if json_mode {
        eprintln!("Isolation: {}", guard.isolation_info().method);
    } else {
        println!("Isolation: {}", guard.isolation_info().method);
    }

    let exit_code = guard.run(&mut state, &cmd, &env, interactive)?;
    containers.save(&state)?;

    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "container_id": state.id,
                "image": image,
                "exit_code": exit_code,
                "status": format!("{:?}", state.status)
            }))?
        );
    } else if exit_code == 0 {
        println!("{} {} exited with code {}", "Container".green(), state.id.bold(), exit_code.to_string().green());
    } else {
        println!("{} {} exited with code {}", "Container".red(), state.id.bold(), exit_code.to_string().red());
    }
    Ok(())
}

/// Extract a Cell-native layer (JSON with dest + files) into the rootfs.
fn extract_cell_layer(data: &[u8], rootfs: &Path) -> Result<usize> {
    let entry: LayerEntry = serde_json::from_slice(data)
        .context("invalid cell layer format")?;

    let dest = entry.dest.trim_start_matches('/').trim_start_matches('\\');

    if entry.files.len() == 1 && !entry.dest.ends_with('/') {
        // Single file with an explicit destination path (e.g., copy "x.txt" to "/hello.txt")
        // Treat dest as the full file path, not a directory.
        let file_path = rootfs.join(dest);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file_path, &entry.files[0].data)?;
    } else {
        // Multiple files or dest ends with / — treat dest as a directory.
        let dest_dir = rootfs.join(dest);
        std::fs::create_dir_all(&dest_dir)?;

        for file in &entry.files {
            let file_path = dest_dir.join(&file.path);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file_path, &file.data)?;
        }
    }

    Ok(entry.files.len())
}
