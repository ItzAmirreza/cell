use anyhow::Result;

pub fn pull(reference: &str) -> Result<()> {
    let name = cell_oci::pull::pull_image(reference)?;
    println!("Successfully pulled: {}", name);
    Ok(())
}
