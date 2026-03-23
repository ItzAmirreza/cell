use anyhow::{Context, Result};
use cell_store::{ContainerStore, ImageStore};
use colored::Colorize;

use super::cell_home;

pub fn execute(id: &str, command: &str, interactive: bool) -> Result<()> {
    let home = cell_home();
    let containers = ContainerStore::new(home.join("containers"));
    let images = ImageStore::new(home.join("store").join("images"));

    let json_mode = super::is_json();

    // Find the container
    let state = containers.get(id)?;
    if json_mode {
        eprintln!("Attaching to container {} (image: '{}')", state.id, state.image);
    } else {
        println!("{} container {} (image: '{}')", "Attaching to".cyan(), state.id.bold(), state.image.bold());
    }

    // Load the image manifest to get config (env, ports, volumes, limits)
    let manifest = images
        .load(&state.image)
        .with_context(|| format!("image '{}' not found for container {}", state.image, state.id))?;

    let env: Vec<(String, String)> = manifest
        .config
        .env
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect();

    // Create a new mutable state for this exec session (reuses the same rootfs)
    let mut exec_state = state.clone();

    let guard = cell_runtime::create_guard(&manifest);
    let exit_code = guard.run(&mut exec_state, command, &env, interactive)?;

    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "container_id": exec_state.id,
                "exit_code": exit_code
            }))?
        );
    } else if exit_code == 0 {
        println!("Exec in container {} exited with code {}", exec_state.id.bold(), exit_code.to_string().green());
    } else {
        println!("Exec in container {} exited with code {}", exec_state.id.bold(), exit_code.to_string().red());
    }
    Ok(())
}
