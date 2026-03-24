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

    let nix_pid = nix::unistd::Pid::from_raw(pid as i32);

    // Send SIGTERM for graceful shutdown.
    println!("Sending SIGTERM to container {} (pid {})...", &state.id[..8], pid);
    let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGTERM);

    // Wait up to 5 seconds for the process to exit.
    let mut exited = false;
    for _ in 0..50 {
        match nix::sys::wait::waitpid(nix_pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)) {
            Ok(nix::sys::wait::WaitStatus::Exited(..)) | Ok(nix::sys::wait::WaitStatus::Signaled(..)) => {
                exited = true;
                break;
            }
            Err(nix::errno::Errno::ECHILD) => {
                // Process already gone.
                exited = true;
                break;
            }
            _ => {}
        }

        // Also check if the process still exists via kill(0).
        if nix::sys::signal::kill(nix_pid, None).is_err() {
            exited = true;
            break;
        }

        thread::sleep(Duration::from_millis(100));
    }

    if !exited {
        // Force kill after timeout.
        println!("Process did not exit, sending SIGKILL...");
        let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGKILL);
        // Reap the zombie.
        let _ = nix::sys::wait::waitpid(nix_pid, None);
    }

    state.status = ContainerStatus::Stopped;
    state.pid = None;

    store
        .update(&state)
        .with_context(|| format!("failed to update container {}", state.id))?;

    println!("Stopped container {}", state.id);
    Ok(())
}
