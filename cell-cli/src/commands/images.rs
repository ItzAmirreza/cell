use anyhow::Result;
use cell_store::ImageStore;
use colored::Colorize;

use super::cell_home;

pub fn execute() -> Result<()> {
    let images = ImageStore::new(cell_home().join("store").join("images"));
    let names = images.list()?;

    if super::is_json() {
        let mut arr = Vec::new();
        for name in &names {
            match images.load(name) {
                Ok(manifest) => {
                    arr.push(serde_json::json!({
                        "name": manifest.name,
                        "created_at": manifest.created_at,
                        "layers": manifest.layers.len()
                    }));
                }
                Err(_) => {
                    arr.push(serde_json::json!({
                        "name": name,
                        "created_at": null,
                        "layers": null
                    }));
                }
            }
        }
        println!("{}", serde_json::to_string(&arr)?);
        return Ok(());
    }

    if names.is_empty() {
        println!("No images found. Use 'cell build' or 'cell pull' to create one.");
        return Ok(());
    }

    println!("{:<30} {:<25} {}", "NAME", "CREATED", "LAYERS");
    println!("{}", "-".repeat(65));

    for name in &names {
        match images.load(name) {
            Ok(manifest) => {
                // Truncate the timestamp for display
                let created = if manifest.created_at.len() > 19 {
                    &manifest.created_at[..19]
                } else {
                    &manifest.created_at
                };
                println!(
                    "{:<30} {:<25} {}",
                    manifest.name.bold(),
                    created,
                    manifest.layers.len()
                );
            }
            Err(_) => {
                println!("{:<30} {:<25} {}", name, "???", "???");
            }
        }
    }

    Ok(())
}
