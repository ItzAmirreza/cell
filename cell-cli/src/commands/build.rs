use anyhow::{Context, Result};
use cell_format::{ContentRef, ImageConfig, ImageManifest, Parser};
use cell_store::{BlobStore, ImageStore};
use colored::Colorize;

use super::cell_home;

/// A layer entry: destination path + raw file data, serialized into each blob.
#[derive(serde::Serialize, serde::Deserialize)]
struct LayerEntry {
    dest: String,
    files: Vec<LayerFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LayerFile {
    path: String,
    data: Vec<u8>,
}

pub fn execute(path: &str) -> Result<()> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Cellfile at '{path}'"))?;

    let spec = Parser::parse(&source)?;

    let json_mode = super::is_json();
    if json_mode {
        eprintln!("Building image '{}'...", spec.name);
    } else {
        println!("Building image '{}'...", spec.name);
    }

    let home = cell_home();
    let blobs = BlobStore::new(home.join("store").join("blobs"));
    let images = ImageStore::new(home.join("store").join("images"));

    let mut layers = Vec::new();
    let mut base_env = Vec::new();
    let mut base_entrypoint = None;
    let mut base_ports = Vec::new();

    // If there's a base image, pull it (or load from cache) and inherit its layers
    if let Some(ref base) = spec.base {
        // Check if we already have this image locally
        let safe_name = base.replace('/', "_").replace(':', "_");
        let base_manifest = match images.load(&safe_name) {
            Ok(m) => {
                if json_mode {
                    eprintln!("  base: {} (cached)", base);
                } else {
                    println!("  base: {} (cached)", base);
                }
                m
            }
            Err(_) => {
                // Pull the base image
                if json_mode {
                    eprintln!("  base: {} (pulling...)", base);
                } else {
                    println!("  base: {} (pulling...)", base);
                }
                let pulled_name = cell_oci::pull::pull_image(base)?;
                images.load(&pulled_name)?
            }
        };

        // Inherit base image layers
        for layer in &base_manifest.layers {
            layers.push(layer.clone());
        }
        if json_mode {
            eprintln!("  inherited {} layer(s) from base", base_manifest.layers.len());
        } else {
            println!(
                "  inherited {} layer(s) from base",
                base_manifest.layers.len()
            );
        }

        // Inherit base config (env, entrypoint, ports)
        base_env = base_manifest.config.env;
        base_entrypoint = base_manifest.config.entrypoint;
        base_ports = base_manifest.config.exposed_ports;
    }

    // Add layers from fs operations
    for op in &spec.fs_ops {
        match op {
            cell_format::FsOp::Copy { src, dest } => {
                let src_path = std::path::Path::new(path)
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(src);

                let files = if src_path.is_file() {
                    let data = std::fs::read(&src_path)
                        .with_context(|| format!("failed to read '{}'", src_path.display()))?;
                    vec![LayerFile {
                        path: src_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
                        data,
                    }]
                } else if src_path.is_dir() {
                    let mut files = Vec::new();
                    collect_files(&src_path, &src_path, &mut files)?;
                    files
                } else {
                    if json_mode {
                        eprintln!("  warning: source '{}' not found, skipping", src);
                    } else {
                        println!("  {}: source '{}' not found, skipping", "warning".yellow(), src);
                    }
                    continue;
                };

                let entry = LayerEntry {
                    dest: dest.clone(),
                    files,
                };

                let blob_data = serde_json::to_vec(&entry)?;
                let digest = blobs.put(&blob_data)?;
                let file_count = entry.files.len();
                if json_mode {
                    eprintln!(
                        "  copy {} -> {} [{}, {} file(s)]",
                        src,
                        dest,
                        &digest[..20],
                        file_count
                    );
                } else {
                    println!(
                        "  copy {} -> {} [{}, {} file(s)]",
                        src,
                        dest,
                        &digest[..20],
                        file_count
                    );
                }
                layers.push(ContentRef {
                    digest,
                    size: blob_data.len() as u64,
                    media_type: "application/cell.layer.v1+json".into(),
                });
            }
        }
    }

    // Merge env: base env + spec env (spec overrides base)
    let mut final_env = base_env;
    for var in &spec.env {
        if let Some(existing) = final_env.iter_mut().find(|e| e.key == var.key) {
            existing.value = var.value.clone();
        } else {
            final_env.push(var.clone());
        }
    }

    // Entrypoint: spec overrides base
    let entrypoint = spec.run.clone().or(base_entrypoint);

    // Ports: merge
    let mut ports = base_ports;
    for p in &spec.expose {
        if !ports.contains(p) {
            ports.push(*p);
        }
    }

    let manifest = ImageManifest {
        name: spec.name.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        config: ImageConfig {
            env: final_env,
            entrypoint,
            exposed_ports: ports,
            workdir: None,
        },
        layers,
        limits: spec.limits.clone(),
        ports: spec.ports.clone(),
        volumes: spec.volumes.clone(),
    };

    images.save(&manifest)?;

    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "name": manifest.name,
                "layers": manifest.layers.len(),
                "status": "success"
            }))?
        );
    } else {
        println!(
            "{} '{}' built successfully ({} layers).",
            "Image".green(),
            spec.name.bold(),
            manifest.layers.len()
        );
    }

    Ok(())
}

fn collect_files(
    base: &std::path::Path,
    dir: &std::path::Path,
    files: &mut Vec<LayerFile>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, files)?;
        } else {
            let relative = path.strip_prefix(base)?.to_string_lossy().to_string();
            let data = std::fs::read(&path)?;
            files.push(LayerFile {
                path: relative,
                data,
            });
        }
    }
    Ok(())
}
