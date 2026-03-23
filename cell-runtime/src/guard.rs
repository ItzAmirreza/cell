use anyhow::Result;
use cell_store::ContainerState;

/// The core isolation abstraction. Each OS provides its own implementation
/// that intercepts syscalls between the contained process and the kernel.
pub trait Guard: Send {
    /// Launch a process inside the guard and return its exit code.
    /// The guard intercepts and rewrites syscalls to provide isolation.
    /// If `interactive` is true, stdin is piped from the host to the container.
    fn run(
        &self,
        state: &mut ContainerState,
        command: &str,
        env: &[(String, String)],
        interactive: bool,
    ) -> Result<i32>;

    /// Stop a running guarded process.
    fn stop(&self, state: &mut ContainerState) -> Result<()>;

    /// Return a human-readable description of the isolation level.
    fn isolation_info(&self) -> IsolationInfo;
}

/// Describes what isolation capabilities are available on this platform.
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
    /// Full kernel-enforced isolation.
    Full,
    /// Partial isolation via syscall interception.
    Intercepted,
    /// No isolation available.
    None,
}

impl std::fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IsolationLevel::Full => write!(f, "full"),
            IsolationLevel::Intercepted => write!(f, "intercepted"),
            IsolationLevel::None => write!(f, "none"),
        }
    }
}

impl std::fmt::Display for IsolationInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Platform:   {}", self.platform)?;
        writeln!(f, "Method:     {}", self.method)?;
        writeln!(f, "Filesystem: {}", self.filesystem)?;
        writeln!(f, "Process:    {}", self.process)?;
        writeln!(f, "Network:    {}", self.network)?;
        write!(f, "Resources:  {}", self.resources)
    }
}
