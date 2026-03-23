use anyhow::Result;
use cell_store::{ContainerState, ContainerStatus};

use crate::guard::{Guard, IsolationInfo, IsolationLevel};

/// Linux implementation of Cell Guard using ptrace + seccomp-BPF.
///
/// Uses ptrace(PTRACE_SYSCALL) to intercept syscalls from the contained
/// process. seccomp-BPF is used to fast-path safe syscalls (read, write)
/// and only trap the ones we need to rewrite (open, connect, etc.).
pub struct LinuxGuard;

impl LinuxGuard {
    pub fn new() -> Self {
        Self
    }
}

impl Guard for LinuxGuard {
    fn run(
        &self,
        state: &mut ContainerState,
        command: &str,
        env: &[(String, String)],
        _interactive: bool,
    ) -> Result<i32> {
        state.status = ContainerStatus::Running;

        println!("[cell-guard:linux] starting process: {command}");
        println!("[cell-guard:linux] rootfs: {:?}", state.rootfs_path);
        println!("[cell-guard:linux] env vars: {}", env.len());

        // TODO: Implement Linux ptrace interception
        // 1. Fork child process
        // 2. Child: install seccomp-BPF filter, then execvp
        // 3. Parent: ptrace attach, PTRACE_SYSCALL loop
        // 4. Intercept open/openat → rewrite paths to rootfs
        // 5. Intercept connect/bind → filter by allowed ports
        // 6. Intercept getpid → return fake PID

        println!("[cell-guard:linux] process exited (stub)");
        state.status = ContainerStatus::Stopped;
        state.pid = None;

        Ok(0)
    }

    fn stop(&self, state: &mut ContainerState) -> Result<()> {
        println!("[cell-guard:linux] stopping container {}", state.id);
        state.status = ContainerStatus::Stopped;
        state.pid = None;
        Ok(())
    }

    fn isolation_info(&self) -> IsolationInfo {
        IsolationInfo {
            platform: "Linux".into(),
            method: "ptrace + seccomp-BPF".into(),
            filesystem: IsolationLevel::Full,
            process: IsolationLevel::Full,
            network: IsolationLevel::Full,
            resources: IsolationLevel::Full,
        }
    }
}
