use std::fmt;

use anyhow::Result;
use cell_store::ContainerState;

/// The core isolation abstraction. Each OS provides its own implementation
/// that intercepts syscalls between the contained process and the kernel.
pub trait Guard: Send {
    /// Launch a process inside the guard and return its exit code.
    fn run(
        &self,
        state: &mut ContainerState,
        command: &str,
        env: &[(String, String)],
    ) -> Result<i32>;

    /// Stop a running container.
    fn stop(&self, state: &mut ContainerState) -> Result<()>;

    /// Report what isolation this platform provides.
    fn isolation_info(&self) -> IsolationInfo;
}

/// Resource limits to enforce on the contained process.
#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    /// Maximum memory in bytes (0 = unlimited).
    pub memory_bytes: u64,
    /// Maximum number of processes (0 = unlimited).
    pub max_processes: u32,
}

#[derive(Debug, Clone)]
pub struct IsolationInfo {
    pub platform: String,
    pub method: String,
    pub filesystem: IsolationLevel,
    pub process: IsolationLevel,
    pub network: IsolationLevel,
    pub resources: IsolationLevel,
}

#[derive(Debug, Clone)]
pub enum IsolationLevel {
    Full,
    Intercepted,
    Partial,
    None,
}

impl fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => write!(f, "full (kernel-enforced)"),
            Self::Intercepted => write!(f, "intercepted (syscall-level)"),
            Self::Partial => write!(f, "partial"),
            Self::None => write!(f, "none"),
        }
    }
}

impl fmt::Display for IsolationInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Platform:   {}", self.platform)?;
        writeln!(f, "Method:     {}", self.method)?;
        writeln!(f, "Filesystem: {}", self.filesystem)?;
        writeln!(f, "Process:    {}", self.process)?;
        writeln!(f, "Network:    {}", self.network)?;
        write!(f, "Resources:  {}", self.resources)
    }
}
