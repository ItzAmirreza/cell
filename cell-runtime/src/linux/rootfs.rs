use std::path::Path;

use anyhow::Result;

/// Prepare a rootfs directory from extracted image layers.
///
/// Each layer is applied in order, with later layers overwriting earlier ones
/// (union-mount semantics without an actual union filesystem).
pub fn prepare_rootfs(layers: &[impl AsRef<Path>], target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;

    for layer in layers {
        let layer = layer.as_ref();
        if layer.is_dir() {
            copy_dir_recursive(layer, target)?;
        }
    }

    // Create essential directories that processes expect
    for dir in &["proc", "sys", "dev", "tmp", "etc", "var", "run"] {
        let p = target.join(dir);
        if !p.exists() {
            std::fs::create_dir_all(&p)?;
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());

        if path.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir_recursive(&path, &target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}
