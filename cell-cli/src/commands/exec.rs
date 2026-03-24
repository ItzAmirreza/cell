use anyhow::{Context, Result};

use cell_runtime;
use cell_store::{ContainerStore, ImageStore};

use super::cell_home;

pub fn exec(id: &str, command: &str) -> Result<()> {
    let home = cell_home();
    let container_store = ContainerStore::with_root(home.clone())?;
    let image_store = ImageStore::new(home.join("images"))?;

    // Load the container state by id or prefix.
    let mut state = container_store
        .get(id)
        .with_context(|| format!("container not found: {id}"))?;

    // Load the image manifest to get env vars.
    let image = &state.image;
    let manifest = image_store.load(image).or_else(|_| {
        let safe = image.replace('/', "_").replace(':', "_");
        if safe != *image {
            image_store.load(&safe)
        } else {
            Err(anyhow::anyhow!("image not found: {image}"))
        }
    }).with_context(|| format!("image not found for container: {image}"))?;

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

    // Create a guard and run the command in the existing container rootfs.
    let guard = cell_runtime::create_guard();

    println!(
        "Executing '{}' in container {}...",
        command,
        &state.id[..8]
    );

    let exit_code = guard.run(&mut state, command, &env)?;

    // Persist updated state.
    container_store.update(&state)?;

    println!(
        "Container {} exec exited with code {}",
        &state.id[..8],
        exit_code
    );
    Ok(())
}
