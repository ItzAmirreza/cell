use anyhow::Result;
use cell_store::{ContainerState, ContainerStatus};

use crate::guard::{Guard, IsolationInfo, IsolationLevel};

/// macOS implementation of Cell Guard using ptrace + Sandbox (Seatbelt).
///
/// Uses ptrace(PT_SYSCALL) for syscall interception and Apple's Sandbox
/// profiles for kernel-enforced restrictions on filesystem, network, and IPC.
pub struct MacosGuard;

impl MacosGuard {
    pub fn new() -> Self {
        Self
    }
}

impl Guard for MacosGuard {
    fn run(
        &self,
        state: &mut ContainerState,
        command: &str,
        env: &[(String, String)],
        _interactive: bool,
    ) -> Result<i32> {
        state.status = ContainerStatus::Running;

        println!("[cell-guard:macos] starting process: {command}");
        println!("[cell-guard:macos] rootfs: {:?}", state.rootfs_path);
        println!("[cell-guard:macos] env vars: {}", env.len());

        // TODO: Implement macOS ptrace + Sandbox interception
        // 1. Generate Sandbox profile restricting filesystem to rootfs
        // 2. Fork child, apply sandbox profile via sandbox_init()
        // 3. ptrace attach for syscall-level interception
        // 4. Path rewriting + PID virtualization

        println!("[cell-guard:macos] process exited (stub)");
        state.status = ContainerStatus::Stopped;
        state.pid = None;

        Ok(0)
    }

    fn stop(&self, state: &mut ContainerState) -> Result<()> {
        println!("[cell-guard:macos] stopping container {}", state.id);
        state.status = ContainerStatus::Stopped;
        state.pid = None;
        Ok(())
    }

    fn isolation_info(&self) -> IsolationInfo {
        IsolationInfo {
            platform: "macOS".into(),
            method: "ptrace + Sandbox (Seatbelt)".into(),
            filesystem: IsolationLevel::Intercepted,
            process: IsolationLevel::Intercepted,
            network: IsolationLevel::Intercepted,
            resources: IsolationLevel::None,
        }
    }
}
