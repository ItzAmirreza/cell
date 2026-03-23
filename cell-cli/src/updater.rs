//! Auto-update system for the Cell CLI.
//!
//! Checks GitHub Releases for newer versions. On startup, runs a non-blocking
//! check (cached for 24h). `cell upgrade` forces an immediate check + download.
//!
//! The binary replaces itself by renaming the old one and writing the new one.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// GitHub owner/repo for release checks. Change this to your repo.
const GITHUB_REPO: &str = "ItzAm/cell";

/// How often to check for updates (in seconds). 24 hours.
const CHECK_INTERVAL: u64 = 86400;

/// Current version of the CLI (from Cargo.toml).
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Cached update state stored at ~/.cell/update-state.json
#[derive(Serialize, Deserialize, Default)]
struct UpdateState {
    /// Unix timestamp of last check.
    last_check: u64,
    /// Latest version found (e.g., "0.2.0").
    latest_version: Option<String>,
    /// Download URL for the latest binary.
    download_url: Option<String>,
    /// Whether an update was applied this session (for display).
    just_updated_from: Option<String>,
}

/// GitHub Release API response (minimal).
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

fn state_path() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".cell")
        .join("update-state.json")
}

fn load_state() -> UpdateState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(state: &UpdateState) {
    let _ = fs::create_dir_all(state_path().parent().unwrap());
    let _ = fs::write(state_path(), serde_json::to_string_pretty(state).unwrap());
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Compare two semver strings. Returns true if `latest` is newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let s = s.trim_start_matches('v');
        let parts: Vec<&str> = s.split('.').collect();
        let major = parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(latest) > parse(current)
}

/// Check GitHub for the latest release. Returns (version, download_url) if newer.
fn check_github() -> Result<Option<(String, String)>> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");

    let client = reqwest::blocking::Client::builder()
        .user_agent("cell-updater")
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client.get(&url).send();

    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(None), // Network error or no releases — silently skip
    };

    let release: GitHubRelease = match resp.json() {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    let version = release.tag_name.trim_start_matches('v').to_string();

    if !is_newer(CURRENT_VERSION, &version) {
        return Ok(None);
    }

    // Find the right binary for this platform
    let target_name = if cfg!(target_os = "windows") {
        "cell-x86_64-windows.exe"
    } else if cfg!(target_os = "linux") {
        "cell-x86_64-linux"
    } else if cfg!(target_os = "macos") {
        "cell-x86_64-macos"
    } else {
        return Ok(None);
    };

    // Also accept generic names
    let asset = release.assets.iter().find(|a| {
        a.name == target_name
            || a.name == "cell.exe"
            || a.name == "cell"
            || a.name.contains(std::env::consts::OS)
    });

    match asset {
        Some(a) => Ok(Some((version, a.browser_download_url.clone()))),
        None => Ok(Some((version, String::new()))), // Version known but no binary
    }
}

/// Download a binary from a URL and replace the current executable.
fn download_and_replace(url: &str) -> Result<()> {
    if url.is_empty() {
        anyhow::bail!("no download URL available for this platform");
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("cell-updater")
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let resp = client.get(url).send().context("download failed")?;

    if !resp.status().is_success() {
        anyhow::bail!("download returned {}", resp.status());
    }

    let bytes = resp.bytes()?;
    let current_exe = std::env::current_exe().context("can't find current exe")?;

    // On Windows: rename current exe to .old, write new one, delete .old on next run
    // On Unix: write to temp, rename over current (atomic)
    if cfg!(target_os = "windows") {
        let old_path = current_exe.with_extension("exe.old");
        let _ = fs::remove_file(&old_path); // clean up any previous .old
        fs::rename(&current_exe, &old_path).context("failed to rename current binary")?;
        fs::write(&current_exe, &bytes).context("failed to write new binary")?;
    } else {
        let tmp_path = current_exe.with_extension("new");
        fs::write(&tmp_path, &bytes).context("failed to write temp binary")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))?;
        }

        fs::rename(&tmp_path, &current_exe).context("failed to replace binary")?;
    }

    Ok(())
}

/// Called on every CLI invocation. Non-blocking check with 24h cache.
/// Prints a message if an update was just applied or is available.
pub fn startup_check() {
    let mut state = load_state();

    // If we just updated, show the message once
    if let Some(ref old_ver) = state.just_updated_from {
        eprintln!(
            "\x1b[32m  Updated cell: v{} -> v{}\x1b[0m",
            old_ver, CURRENT_VERSION
        );
        state.just_updated_from = None;
        save_state(&state);
        return;
    }

    // Clean up .old binary from previous update (Windows)
    if cfg!(target_os = "windows") {
        if let Ok(exe) = std::env::current_exe() {
            let old = exe.with_extension("exe.old");
            let _ = fs::remove_file(old);
        }
    }

    // Check if enough time has passed since last check
    let now = now_secs();
    if now - state.last_check < CHECK_INTERVAL {
        // If we know there's an update but haven't applied it, remind the user
        if let Some(ref ver) = state.latest_version {
            if is_newer(CURRENT_VERSION, ver) {
                eprintln!(
                    "\x1b[33m  Update available: v{} -> v{} (run `cell upgrade`)\x1b[0m",
                    CURRENT_VERSION, ver
                );
            }
        }
        return;
    }

    // Do the check in the background (spawn a thread so it doesn't block)
    std::thread::spawn(move || {
        if let Ok(Some((version, url))) = check_github() {
            let mut state = load_state();
            state.last_check = now_secs();
            state.latest_version = Some(version);
            state.download_url = if url.is_empty() { None } else { Some(url) };
            save_state(&state);
        } else {
            // Update the check timestamp even on failure so we don't retry immediately
            let mut state = load_state();
            state.last_check = now_secs();
            save_state(&state);
        }
    });
}

/// Manual upgrade command: check + download + replace immediately.
pub fn upgrade() -> Result<()> {
    eprintln!("Checking for updates...");
    eprintln!("Current version: v{}", CURRENT_VERSION);

    let result = check_github()?;

    match result {
        None => {
            eprintln!("\x1b[32mAlready up to date.\x1b[0m");
            Ok(())
        }
        Some((version, url)) => {
            eprintln!("New version available: v{}", version);

            if url.is_empty() {
                eprintln!("\x1b[33mNo binary available for this platform in the release.\x1b[0m");
                eprintln!("Build from source: cargo build --release");
                return Ok(());
            }

            eprintln!("Downloading...");
            download_and_replace(&url)?;

            // Save state so next run shows the "Updated" message
            let mut state = load_state();
            state.just_updated_from = Some(CURRENT_VERSION.to_string());
            state.latest_version = Some(version.clone());
            state.last_check = now_secs();
            save_state(&state);

            eprintln!(
                "\x1b[32mUpdated: v{} -> v{}\x1b[0m",
                CURRENT_VERSION, version
            );
            Ok(())
        }
    }
}

/// Get current version string.
#[allow(dead_code)]
pub fn version() -> &'static str {
    CURRENT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.0", "1.0.0"));
        assert!(!is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn test_is_newer_with_v_prefix() {
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(is_newer("v0.1.0", "0.2.0"));
    }
}
