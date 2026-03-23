use anyhow::Result;
use cell_store::{ContainerStatus, ContainerStore};
use colored::Colorize;

use super::cell_home;

pub fn execute(id: &str) -> Result<()> {
    let containers = ContainerStore::new(cell_home().join("containers"));
    let mut state = containers.get(id)?;
    let json_mode = super::is_json();

    if let Some(pid) = state.pid {
        if json_mode {
            eprintln!("Stopping container {} (PID {})...", state.id, pid);
        } else {
            println!("Stopping container {} (PID {})...", state.id.bold(), pid);
        }
        kill_pid(pid);
    } else if !json_mode {
        println!(
            "{}: container {} has no PID, marking as stopped",
            "note".yellow(),
            state.id.bold()
        );
    }

    state.status = ContainerStatus::Stopped;
    state.pid = None;
    containers.save(&state)?;

    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "id": state.id,
                "status": "stopped"
            }))?
        );
    } else {
        println!("Container {} {}", state.id.bold(), "stopped".green());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn kill_pid(pid: u32) {
    use std::ffi::c_void;

    const PROCESS_TERMINATE: u32 = 0x0001;

    extern "system" {
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> *mut c_void;
        fn TerminateProcess(h_process: *mut c_void, u_exit_code: u32) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn kill_pid(_pid: u32) {
    // Non-Windows: not implemented (cell targets Windows).
}
