use anyhow::Result;

pub fn info() -> Result<()> {
    let guard = cell_runtime::create_guard();
    let info = guard.isolation_info();
    println!("{}", info);
    Ok(())
}
