use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use cell_store::{ContainerStatus, ContainerStore};

use super::cell_home;

pub fn stop(id: &str) -> Result<()> {
    let home = cell_home();
    let store = ContainerStore::with_root(home)?;

    let mut state = store
        .get(id)
        .with_context(|| format!("container not found: {id}"))?;

    let pid = match state.pid {
        Some(p) => p,
        None => bail!("container {} is not running", state.id),
    };

    println!("Sending SIGTERM to container {} (pid {})...", &state.id[..8], pid);

    let exited = platform_stop(pid);

    if !exited {
        println!("Process did not exit, sending SIGKILL...");
        platform_kill(pid);
    }

    state.status = ContainerStatus::Stopped;
    state.pid = None;

    store
        .update(&state)
        .with_context(|| format!("failed to update container {}", state.id))?;

    println!("Stopped container {}", state.id);
    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_stop(pid: u32) -> bool {
    let nix_pid = nix::unistd::Pid::from_raw(pid as i32);
    let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGTERM);

    for _ in 0..50 {
        match nix::sys::wait::waitpid(nix_pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
            Ok(nix::sys::wait::WaitStatus::Exited(..))
            | Ok(nix::sys::wait::WaitStatus::Signaled(..)) => return true,
            Err(nix::errno::Errno::ECHILD) => return true,
            _ => {}
        }
        if nix::sys::signal::kill(nix_pid, None).is_err() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(target_os = "linux")]
fn platform_kill(pid: u32) {
    let nix_pid = nix::unistd::Pid::from_raw(pid as i32);
    let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGKILL);
    let _ = nix::sys::wait::waitpid(nix_pid, None);
}

#[cfg(target_os = "windows")]
fn platform_stop(pid: u32) -> bool {
    // On Windows, use TerminateProcess via the windows crate
    // For now, just mark as stopped — the Job Object's KILL_ON_JOB_CLOSE handles cleanup
    unsafe {
        let handle = windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_TERMINATE,
            false,
            pid,
        );
        if let Ok(handle) = handle {
            let _ = windows::Win32::System::Threading::TerminateProcess(handle, 1);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn platform_kill(pid: u32) {
    platform_stop(pid);
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn platform_stop(_pid: u32) -> bool {
    eprintln!("stop not implemented on this platform");
    true
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn platform_kill(_pid: u32) {}
