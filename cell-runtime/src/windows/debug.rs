use anyhow::Result;
use cell_store::{ContainerState, ContainerStatus};

use crate::guard::{Guard, IsolationInfo, IsolationLevel};

/// Windows stub — real implementation exists on the Windows build.
pub struct WindowsGuard;

impl WindowsGuard {
    pub fn new() -> Self { Self }
}

impl Guard for WindowsGuard {
    fn run(&self, state: &mut ContainerState, command: &str, env: &[(String, String)]) -> Result<i32> {
        state.status = ContainerStatus::Running;
        println!("[cell-guard:windows] would start: {command}");
        println!("[cell-guard:windows] env vars: {}", env.len());
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
            platform: "Windows".into(),
            method: "Debug API + Job Objects (stub on this build)".into(),
            filesystem: IsolationLevel::None,
            process: IsolationLevel::None,
            network: IsolationLevel::None,
            resources: IsolationLevel::None,
        }
    }
}
