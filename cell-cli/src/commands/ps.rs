use anyhow::Result;
use cell_store::{ContainerStatus, ContainerStore};
use colored::Colorize;

use super::cell_home;

pub fn execute() -> Result<()> {
    let containers = ContainerStore::new(cell_home().join("containers"));
    let list = containers.list()?;

    if super::is_json() {
        let arr: Vec<serde_json::Value> = list
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "image": c.image,
                    "status": format!("{:?}", c.status),
                    "pid": c.pid,
                    "created_at": c.created_at
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&arr)?);
        return Ok(());
    }

    if list.is_empty() {
        println!("No containers found.");
        return Ok(());
    }

    println!(
        "{:<12} {:<20} {:<12} {:<10} {}",
        "CONTAINER", "IMAGE", "STATUS", "PID", "CREATED"
    );
    println!("{}", "-".repeat(70));

    for c in &list {
        let created = if c.created_at.len() > 19 {
            &c.created_at[..19]
        } else {
            &c.created_at
        };
        let status_str = match c.status {
            ContainerStatus::Running => format!("{:?}", c.status).green().to_string(),
            ContainerStatus::Stopped => format!("{:?}", c.status).red().to_string(),
            ContainerStatus::Created => format!("{:?}", c.status).cyan().to_string(),
        };
        println!(
            "{:<12} {:<20} {:<12} {:<10} {}",
            c.id.bold(),
            &c.image,
            status_str,
            c.pid.map_or("-".to_string(), |p| p.to_string()),
            created,
        );
    }

    Ok(())
}
