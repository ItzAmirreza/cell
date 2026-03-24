pub mod guard;
pub mod syscall;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

pub use guard::{Guard, IsolationInfo, ResourceLimits};

/// Create the platform-appropriate guard.
pub fn create_guard() -> Box<dyn Guard> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxGuard::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsGuard::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacosGuard::new())
    }
}

/// Create a guard with resource limits.
pub fn create_guard_with_limits(limits: ResourceLimits) -> Box<dyn Guard> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxGuard::with_limits(limits))
    }
    #[cfg(target_os = "windows")]
    {
        let _ = limits;
        Box::new(windows::WindowsGuard::new())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = limits;
        Box::new(macos::MacosGuard::new())
    }
}
