use std::path::Path;

use anyhow::Result;

/// Prepare a rootfs directory from extracted image layers.
///
/// Each layer is applied in order, with later layers overwriting earlier ones
/// (union-mount semantics without an actual union filesystem).
pub fn prepare_rootfs(layers: &[impl AsRef<Path>], target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;

    for layer_path in layers {
        // TODO: Extract layer contents into target.
        // For now, layers are stored as directories — copy their contents.
        let layer = layer_path.as_ref();
        if layer.is_dir() {
            copy_dir_recursive(layer, target)?;
        }
    }

    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
