use anyhow::Result;

use cell_store::ContainerStore;

use super::cell_home;

pub fn ps() -> Result<()> {
    let home = cell_home();
    let store = ContainerStore::with_root(home)?;

    let containers = store.list()?;

    if containers.is_empty() {
        println!("No containers found.");
        return Ok(());
    }

    println!(
        "{:<14} {:<24} {:<12} {}",
        "CONTAINER ID", "IMAGE", "STATUS", "CREATED"
    );
    println!("{}", "-".repeat(70));

    for c in &containers {
        let status = format!("{:?}", c.status).to_lowercase();
        let created = format_date(&c.created_at);
        println!(
            "{:<14} {:<24} {:<12} {}",
            &c.id, c.image, status, created,
        );
    }

    Ok(())
}

/// Truncate an RFC 3339 timestamp to a shorter display form.
fn format_date(rfc3339: &str) -> String {
    if rfc3339.len() >= 19 {
        rfc3339[..19].replace('T', " ")
    } else {
        rfc3339.to_string()
    }
}
