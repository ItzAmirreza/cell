use anyhow::Result;
use cell_store::ImageStore;
use colored::Colorize;

use super::cell_home;

pub fn execute(reference: &str) -> Result<()> {
    let image_ref = cell_oci::registry::ImageRef::parse(reference);
    let json_mode = super::is_json();

    if json_mode {
        eprintln!("Pulling {}...", image_ref.full_ref());
    } else {
        println!("{} {}...", "Pulling".cyan(), image_ref.full_ref());
    }

    let name = cell_oci::pull::pull_image(reference)?;

    if json_mode {
        let images = ImageStore::new(cell_home().join("store").join("images"));
        let layer_count = images.load(&name).map(|m| m.layers.len()).unwrap_or(0);
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "name": name,
                "layers": layer_count,
                "status": "success"
            }))?
        );
    } else {
        println!("{} Run with: cell run {}", "Done.".green(), name.bold());
    }
    Ok(())
}
