use anyhow::{Context, Result};

use cell_store::ContainerStore;

use super::cell_home;

pub fn rm(id: &str) -> Result<()> {
    let home = cell_home();
    let store = ContainerStore::with_root(home)?;

    // Resolve the container first to get its full id for display.
    let state = store
        .get(id)
        .with_context(|| format!("container not found: {id}"))?;
    let full_id = state.id.clone();

    store
        .remove(id)
        .with_context(|| format!("failed to remove container {}", full_id))?;

    println!("Removed container {}", full_id);
    Ok(())
}
