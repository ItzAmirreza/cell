use anyhow::Result;
use cell_store::{ContainerState, ContainerStatus};

use crate::guard::{Guard, IsolationInfo, IsolationLevel};

/// macOS stub — will use ptrace(PT_SYSCALL) + Seatbelt.
pub struct MacosGuard;

impl MacosGuard {
    pub fn new() -> Self { Self }
}

impl Guard for MacosGuard {
    fn run(&self, state: &mut ContainerState, command: &str, env: &[(String, String)]) -> Result<i32> {
        state.status = ContainerStatus::Running;
        println!("[cell-guard:macos] would start: {command}");
        println!("[cell-guard:macos] env vars: {}", env.len());
        state.status = ContainerStatus::Stopped;
        Ok(0)
    }

    fn stop(&self, state: &mut ContainerState) -> Result<()> {
        state.status = ContainerStatus::Stopped;
        state.pid = None;
        Ok(())
    }

    fn isolation_info(&self) -> IsolationInfo {
        IsolationInfo {
            platform: "macOS".into(),
            method: "ptrace + Seatbelt (stub)".into(),
            filesystem: IsolationLevel::None,
            process: IsolationLevel::None,
            network: IsolationLevel::None,
            resources: IsolationLevel::None,
        }
    }
}
