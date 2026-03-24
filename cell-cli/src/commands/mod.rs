pub mod build;
pub mod convert;
pub mod exec;
pub mod images;
pub mod info;
pub mod ps;
pub mod pull;
pub mod rm;
pub mod run;
pub mod stop;

use std::path::PathBuf;

/// Return the root of the Cell data directory (`~/.cell`).
pub fn cell_home() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".cell")
}
