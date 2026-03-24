use anyhow::Result;

use cell_store::ImageStore;

use super::cell_home;

pub fn images() -> Result<()> {
    let home = cell_home();
    let image_store = ImageStore::new(home.join("images"))?;

    let names = image_store.list()?;

    if names.is_empty() {
        println!("No images found.");
        return Ok(());
    }

    println!("{:<30} {:<28} {}", "NAME", "CREATED", "LAYERS");
    println!("{}", "-".repeat(68));

    for name in &names {
        match image_store.load(name) {
            Ok(manifest) => {
                let created = format_date(&manifest.created_at);
                println!(
                    "{:<30} {:<28} {}",
                    manifest.name,
                    created,
                    manifest.layers.len(),
                );
            }
            Err(e) => {
                eprintln!("warning: failed to load image '{}': {}", name, e);
            }
        }
    }

    Ok(())
}

/// Truncate an RFC 3339 timestamp to a shorter display form.
fn format_date(rfc3339: &str) -> String {
    // Try to show just "YYYY-MM-DD HH:MM:SS".
    if rfc3339.len() >= 19 {
        rfc3339[..19].replace('T', " ")
    } else {
        rfc3339.to_string()
    }
}
