pub mod guard;
pub mod syscall;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

pub use guard::{Guard, IsolationInfo, IsolationLevel};

pub static VERBOSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn is_verbose() -> bool { VERBOSE.load(std::sync::atomic::Ordering::Relaxed) }

/// Create the appropriate Guard implementation for the current platform,
/// extracting resource limits, port mappings, and volume mounts from the image manifest.
pub fn create_guard(manifest: &cell_format::ImageManifest) -> Box<dyn Guard> {
    #[cfg(target_os = "linux")]
    {
        let _ = manifest;
        Box::new(linux::LinuxGuard::new())
    }

    #[cfg(target_os = "windows")]
    {
        use cell_format::VolumeMount;

        let limits = manifest.limits.as_ref();
        let memory_limit = limits.and_then(|l| l.memory_bytes()).unwrap_or(0);
        let process_limit = limits.and_then(|l| l.processes).unwrap_or(0);

        // Build the NT-format volumes directory: \??\C:\Users\<user>\.cell\volumes\
        let volumes_dir_nt = {
            let home = dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("C:\\Users\\Default"));
            let vol_dir = home.join(".cell").join("volumes");
            format!("\\??\\{}", vol_dir.to_string_lossy())
        };

        // Convert PortMapping vec to (host_port, container_port) tuples.
        let port_mappings: Vec<(u16, u16)> = manifest
            .ports
            .iter()
            .map(|pm| (pm.host, pm.container))
            .collect();

        // Convert VolumeMount vec to (container_path_nt, host_volume_path_nt) tuples.
        // The container sees files at e.g. \??\C:\app\data; we redirect them to
        // \??\C:\Users\<user>\.cell\volumes\<name>\<rest>.
        let volume_mounts: Vec<(String, String)> = manifest
            .volumes
            .iter()
            .map(|vm: &VolumeMount| {
                // container_path is a POSIX-style path like "/app/data".
                // Convert to NT: replace leading / with \??\ + drive letter equivalent.
                // On Windows we treat container paths as relative to C:\.
                let container_nt = format!(
                    "\\??\\C:{}",
                    vm.container_path.replace('/', "\\")
                );
                let host_nt = format!("{}\\{}", volumes_dir_nt, vm.name);
                (container_nt, host_nt)
            })
            .collect();

        let mut guard = if memory_limit > 0 || process_limit > 0 {
            windows::WindowsGuard::with_limits(memory_limit, process_limit)
        } else {
            windows::WindowsGuard::new()
        };
        guard.port_mappings = port_mappings;
        guard.volume_mounts = volume_mounts;
        Box::new(guard)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = manifest;
        Box::new(macos::MacosGuard::new())
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        compile_error!("Unsupported platform. Cell supports Linux, Windows, and macOS.")
    }
}
