use anyhow::Result;
use cell_format::{ImageConfig, ImageManifest};

pub fn execute() -> Result<()> {
    // Create a minimal manifest just to satisfy create_guard's signature.
    let dummy_manifest = ImageManifest {
        name: String::new(),
        created_at: String::new(),
        config: ImageConfig {
            env: vec![],
            entrypoint: None,
            exposed_ports: vec![],
            workdir: None,
        },
        layers: vec![],
        limits: None,
        ports: vec![],
        volumes: vec![],
    };
    let guard = cell_runtime::create_guard(&dummy_manifest);
    let info = guard.isolation_info();

    if super::is_json() {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "platform": info.platform,
                "method": info.method,
                "filesystem": info.filesystem.to_string(),
                "process": info.process.to_string(),
                "network": info.network.to_string(),
                "resources": info.resources.to_string()
            }))?
        );
        return Ok(());
    }

    println!("cell v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("{info}");

    Ok(())
}
