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
use std::sync::atomic::{AtomicBool, Ordering};

static JSON_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_json_mode(enabled: bool) {
    JSON_MODE.store(enabled, Ordering::Relaxed);
}

pub fn is_json() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

/// Get the Cell home directory (~/.cell/).
pub fn cell_home() -> PathBuf {
    dirs::home_dir()
        .expect("could not determine home directory")
        .join(".cell")
}
