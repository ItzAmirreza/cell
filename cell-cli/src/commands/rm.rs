use anyhow::Result;
use cell_store::ContainerStore;

use super::cell_home;

pub fn execute(id: &str) -> Result<()> {
    let containers = ContainerStore::new(cell_home().join("containers"));
    let state = containers.get(id)?;
    let json_mode = super::is_json();

    if !json_mode {
        println!("Removing container {}...", state.id);
    }
    containers.remove(&state.id)?;

    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "id": state.id,
                "status": "removed"
            }))?
        );
    } else {
        println!("Removed.");
    }
    Ok(())
}
