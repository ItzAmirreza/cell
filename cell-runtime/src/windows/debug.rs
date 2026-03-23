use std::ffi::OsStr;
use std::io;
use std::mem;
use std::os::windows::ffi::OsStrExt;

use anyhow::{Context, Result};
use cell_store::{ContainerState, ContainerStatus};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::Debug::*;
use windows::Win32::System::JobObjects::*;
use windows::Win32::System::Threading::*;

use crate::guard::{Guard, IsolationInfo, IsolationLevel};
use super::intercept::Interceptor;

// Raw FFI for pipe creation (simpler than adding more windows crate features)
#[repr(C)]
struct SecurityAttributes {
    n_length: u32,
    lp_security_descriptor: *mut std::ffi::c_void,
    b_inherit_handle: i32,
}

extern "system" {
    fn CreatePipe(
        h_read_pipe: *mut HANDLE,
        h_write_pipe: *mut HANDLE,
        lp_pipe_attributes: *const SecurityAttributes,
        n_size: u32,
    ) -> BOOL;
    fn SetHandleInformation(h_object: HANDLE, dw_mask: u32, dw_flags: u32) -> BOOL;
    fn PeekNamedPipe(
        h_named_pipe: HANDLE,
        lp_buffer: *mut std::ffi::c_void,
        n_buffer_size: u32,
        lp_bytes_read: *mut u32,
        lp_total_bytes_avail: *mut u32,
        lp_bytes_left_this_message: *mut u32,
    ) -> BOOL;
    fn ReadFile(
        h_file: HANDLE,
        lp_buffer: *mut std::ffi::c_void,
        n_number_of_bytes_to_read: u32,
        lp_number_of_bytes_read: *mut u32,
        lp_overlapped: *mut std::ffi::c_void,
    ) -> BOOL;
}

const HANDLE_FLAG_INHERIT: u32 = 0x00000001;
const STARTF_USESTDHANDLES: u32 = 0x00000100;

pub struct WindowsGuard {
    pub memory_limit: usize,
    pub process_limit: u32,
    /// Port mappings: (host_port, container_port). Passed through to the Interceptor so that
    /// bind() calls from the container can be rewritten to use the host-side port.
    pub port_mappings: Vec<(u16, u16)>,
    /// Volume mounts: (container_path_nt, host_volume_path_nt). Passed through to the
    /// Interceptor so that file paths inside the container can be redirected to the volume.
    pub volume_mounts: Vec<(String, String)>,
}

impl WindowsGuard {
    pub fn new() -> Self {
        Self {
            memory_limit: 0,
            process_limit: 0,
            port_mappings: vec![],
            volume_mounts: vec![],
        }
    }

    pub fn with_limits(memory_limit: usize, process_limit: u32) -> Self {
        Self {
            memory_limit,
            process_limit,
            port_mappings: vec![],
            volume_mounts: vec![],
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Create an inheritable pipe. Returns (read_handle, write_handle).
fn create_pipe() -> Result<(HANDLE, HANDLE)> {
    let mut read_handle: HANDLE = HANDLE::default();
    let mut write_handle: HANDLE = HANDLE::default();

    let sa = SecurityAttributes {
        n_length: mem::size_of::<SecurityAttributes>() as u32,
        lp_security_descriptor: std::ptr::null_mut(),
        b_inherit_handle: 1, // inheritable
    };

    let ok = unsafe { CreatePipe(&mut read_handle, &mut write_handle, &sa, 0) };
    if !ok.as_bool() {
        anyhow::bail!("CreatePipe failed: {}", io::Error::last_os_error());
    }

    // The read end should NOT be inherited by the child
    let _ = unsafe { SetHandleInformation(read_handle, HANDLE_FLAG_INHERIT, 0) };

    Ok((read_handle, write_handle))
}

/// Drain all available data from a pipe and write it to stdout.
fn drain_pipe(pipe: HANDLE) {
    let mut buf = [0u8; 4096];
    loop {
        let mut avail: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(pipe, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut())
        };
        if !ok.as_bool() || avail == 0 {
            break;
        }
        let to_read = avail.min(buf.len() as u32);
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(pipe, buf.as_mut_ptr() as *mut _, to_read, &mut read, std::ptr::null_mut())
        };
        if !ok.as_bool() || read == 0 {
            break;
        }
        // Write to our stdout
        let _ = io::Write::write_all(&mut io::stdout(), &buf[..read as usize]);
    }
}

fn create_job_object(memory_limit: usize, process_limit: u32) -> Result<HANDLE> {
    unsafe {
        let job = CreateJobObjectW(None, None)
            .context("CreateJobObjectW failed")?;

        let mut ext_info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
        ext_info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        if memory_limit > 0 {
            ext_info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            ext_info.ProcessMemoryLimit = memory_limit;
        }

        if process_limit > 0 {
            ext_info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            ext_info.BasicLimitInformation.ActiveProcessLimit = process_limit;
        }

        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &ext_info as *const _ as *const std::ffi::c_void,
            mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .context("SetInformationJobObject failed")?;

        Ok(job)
    }
}

fn debug_event_loop(
    pi: &PROCESS_INFORMATION,
    state: &mut ContainerState,
    interceptor: &mut Interceptor,
    stdout_pipe: HANDLE,
) -> Result<i32> {
    let mut exit_code: u32 = 0;
    let mut dll_count: u32 = 0;
    let mut thread_count: u32 = 0;
    let mut initial_breakpoint_seen = false;

    state.pid = Some(pi.dwProcessId);
    state.status = ContainerStatus::Running;

    loop {
        // Drain stdout/stderr from the pipe before waiting for debug events
        drain_pipe(stdout_pipe);

        let mut event: DEBUG_EVENT = unsafe { mem::zeroed() };
        let wait_result = unsafe { WaitForDebugEvent(&mut event, 100) };

        if wait_result.is_err() {
            let still_alive = unsafe {
                let mut ec: u32 = 0;
                let _ = GetExitCodeProcess(pi.hProcess, &mut ec);
                ec == 259
            };
            if !still_alive {
                break;
            }
            continue;
        }

        match event.dwDebugEventCode {
            CREATE_PROCESS_DEBUG_EVENT => {
                let info = unsafe { event.u.CreateProcessInfo };
                if !info.hFile.is_invalid() {
                    let _ = unsafe { CloseHandle(info.hFile) };
                }
            }
            EXIT_PROCESS_DEBUG_EVENT => {
                let info = unsafe { event.u.ExitProcess };
                exit_code = info.dwExitCode;
                // Drain any remaining output
                drain_pipe(stdout_pipe);
                let _ = unsafe {
                    ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE)
                };
                break;
            }
            CREATE_THREAD_DEBUG_EVENT => { thread_count += 1; }
            EXIT_THREAD_DEBUG_EVENT => {}
            LOAD_DLL_DEBUG_EVENT => {
                let info = unsafe { event.u.LoadDll };
                dll_count += 1;
                if !info.hFile.is_invalid() {
                    let _ = unsafe { CloseHandle(info.hFile) };
                }
            }
            UNLOAD_DLL_DEBUG_EVENT => {}
            OUTPUT_DEBUG_STRING_EVENT => {}
            EXCEPTION_DEBUG_EVENT => {
                let info = unsafe { event.u.Exception };
                let code = info.ExceptionRecord.ExceptionCode;
                let addr = info.ExceptionRecord.ExceptionAddress as u64;

                if code == EXCEPTION_BREAKPOINT {
                    if !initial_breakpoint_seen {
                        initial_breakpoint_seen = true;
                        if let Err(e) = interceptor.setup_breakpoints() {
                            if crate::is_verbose() { eprintln!("[cell-guard] warning: could not set breakpoints: {e}"); }
                        }
                        let _ = unsafe {
                            ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE)
                        };
                        continue;
                    }

                    match interceptor.handle_breakpoint(event.dwThreadId, addr) {
                        Ok(true) => {
                            let _ = unsafe {
                                ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE)
                            };
                            continue;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            if crate::is_verbose() { eprintln!("[cell-guard] breakpoint error: {e}"); }
                        }
                    }

                    let _ = unsafe {
                        ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE)
                    };
                    continue;
                }

                if code == NTSTATUS(0x80000004u32 as i32) {
                    match interceptor.handle_single_step(event.dwThreadId) {
                        Ok(true) => {
                            let _ = unsafe {
                                ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE)
                            };
                            continue;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            if crate::is_verbose() { eprintln!("[cell-guard] single-step error: {e}"); }
                        }
                    }
                }

                let _ = unsafe {
                    ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_EXCEPTION_NOT_HANDLED)
                };
                continue;
            }
            RIP_EVENT => {
                if crate::is_verbose() { eprintln!("[cell-guard] RIP event"); }
            }
            _ => {}
        }

        let _ = unsafe {
            ContinueDebugEvent(event.dwProcessId, event.dwThreadId, DBG_CONTINUE)
        };
    }

    // Final drain
    drain_pipe(stdout_pipe);

    interceptor.print_summary();
    eprintln!(
        "[cell-guard] session stats: {} DLLs loaded, {} threads, {} syscalls intercepted",
        dll_count, thread_count, interceptor.intercept_count
    );

    Ok(exit_code as i32)
}

impl Guard for WindowsGuard {
    fn run(
        &self,
        state: &mut ContainerState,
        command: &str,
        env: &[(String, String)],
        interactive: bool,
    ) -> Result<i32> {
        // Build environment block
        let env_block = if !env.is_empty() {
            let mut block = String::new();
            for (k, v) in env {
                block.push_str(&format!("{k}={v}"));
                block.push('\0');
            }
            for var in ["SystemRoot", "PATH", "TEMP", "TMP", "USERPROFILE"] {
                if let Ok(val) = std::env::var(var) {
                    if !env.iter().any(|(k, _)| k == var) {
                        block.push_str(&format!("{var}={val}"));
                        block.push('\0');
                    }
                }
            }
            block.push('\0');
            Some(block)
        } else {
            None
        };

        let working_dir = state.rootfs_path.clone();
        std::fs::create_dir_all(&working_dir)?;
        let working_dir_wide = to_wide(&working_dir.to_string_lossy());

        let job = create_job_object(self.memory_limit, self.process_limit)?;
        if (self.memory_limit > 0 || self.process_limit > 0) && crate::is_verbose() {
            eprintln!(
                "[cell-guard] resource limits: memory={}, processes={}",
                if self.memory_limit > 0 {
                    format!("{}MB", self.memory_limit / 1024 / 1024)
                } else {
                    "unlimited".into()
                },
                if self.process_limit > 0 {
                    self.process_limit.to_string()
                } else {
                    "unlimited".into()
                }
            );
        }

        // Create pipes for stdout and stderr
        let (stdout_read, stdout_write) = create_pipe()?;
        let (stderr_read, stderr_write) = create_pipe()?;

        // When interactive, also create a stdin pipe so the host's stdin can be
        // forwarded into the contained process.
        let stdin_pipe = if interactive {
            Some(create_pipe()?)
        } else {
            None
        };

        // Set up STARTUPINFO with redirected handles
        let mut si: STARTUPINFOW = unsafe { mem::zeroed() };
        si.cb = mem::size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTUPINFOW_FLAGS(STARTF_USESTDHANDLES);
        si.hStdOutput = stdout_write;
        si.hStdError = stderr_write;
        // In interactive mode the child reads from the read end of the stdin pipe;
        // otherwise leave hStdInput null (no stdin for the contained process).
        si.hStdInput = stdin_pipe
            .as_ref()
            .map(|(read, _write)| *read)
            .unwrap_or(HANDLE::default());

        let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        let creation_flags = DEBUG_PROCESS
            | CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT;

        let env_ptr = env_block.as_ref().map(|b| {
            let wide: Vec<u16> = OsStr::new(b).encode_wide().collect();
            wide
        });

        let cmd_wide = to_wide(command);

        let result = unsafe {
            CreateProcessW(
                None,
                PWSTR(cmd_wide.as_ptr() as *mut u16),
                None,
                None,
                true, // inherit handles (for the pipes)
                creation_flags,
                env_ptr.as_ref().map(|w| w.as_ptr() as *const std::ffi::c_void),
                PCWSTR(working_dir_wide.as_ptr()),
                &si,
                &mut pi,
            )
        };

        // Close the write ends of stdout/stderr in the parent — only the child uses them.
        let _ = unsafe { CloseHandle(stdout_write) };
        let _ = unsafe { CloseHandle(stderr_write) };
        // Close the read end of the stdin pipe in the parent — only the child reads from it.
        if let Some((stdin_read, _)) = &stdin_pipe {
            let _ = unsafe { CloseHandle(*stdin_read) };
        }

        if let Err(e) = result {
            let _ = unsafe { CloseHandle(job) };
            let _ = unsafe { CloseHandle(stdout_read) };
            let _ = unsafe { CloseHandle(stderr_read) };
            if let Some((_r, w)) = &stdin_pipe {
                let _ = unsafe { CloseHandle(*w) };
            }
            return Err(anyhow::anyhow!("failed to create process '{}': {}", command, e));
        }

        if let Err(e) = unsafe { AssignProcessToJobObject(job, pi.hProcess) } {
            if crate::is_verbose() { eprintln!("[cell-guard] warning: could not assign to job object: {e}"); }
        }

        let mut interceptor = Interceptor::new(
            pi.hProcess,
            state.rootfs_path.clone(),
            true,
            self.port_mappings.clone(),
            self.volume_mounts.clone(),
        );

        unsafe { ResumeThread(pi.hThread) };

        // Spawn a thread to drain stderr in the background.
        // HANDLE is a raw pointer — safe to send across threads for our use.
        let stderr_raw = stderr_read.0 as usize;
        let stderr_thread = std::thread::spawn(move || {
            let handle = HANDLE(stderr_raw as *mut std::ffi::c_void);
            let mut buf = [0u8; 4096];
            loop {
                let mut read: u32 = 0;
                let ok = unsafe {
                    ReadFile(handle, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read, std::ptr::null_mut())
                };
                if !ok.as_bool() || read == 0 {
                    break;
                }
                let _ = io::Write::write_all(&mut io::stderr(), &buf[..read as usize]);
            }
        });

        // When interactive, spawn a thread that reads from the host's stdin and
        // writes to the write end of the child's stdin pipe.
        // The contained process is inside a Job Object with KILL_ON_JOB_CLOSE, so
        // when Cell exits (even abnormally via Ctrl+C) the job handle closes and
        // every process in the job is killed automatically — no explicit Ctrl+C
        // handler is needed.
        let stdin_thread = stdin_pipe.as_ref().map(|(_, stdin_write)| {
            let stdin_write_raw = stdin_write.0 as usize;
            std::thread::spawn(move || {
                use std::io::Read;
                let pipe_write = HANDLE(stdin_write_raw as *mut std::ffi::c_void);
                let mut buf = [0u8; 4096];
                loop {
                    match io::stdin().read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut written: u32 = 0;
                            // WriteFile via the same FFI surface already in scope
                            extern "system" {
                                fn WriteFile(
                                    h_file: HANDLE,
                                    lp_buffer: *const std::ffi::c_void,
                                    n_number_of_bytes_to_write: u32,
                                    lp_number_of_bytes_written: *mut u32,
                                    lp_overlapped: *mut std::ffi::c_void,
                                ) -> BOOL;
                            }
                            let ok = unsafe {
                                WriteFile(
                                    pipe_write,
                                    buf.as_ptr() as *const _,
                                    n as u32,
                                    &mut written,
                                    std::ptr::null_mut(),
                                )
                            };
                            if !ok.as_bool() {
                                break;
                            }
                        }
                    }
                }
            })
        });

        let exit_code = debug_event_loop(&pi, state, &mut interceptor, stdout_read);

        // Cleanup
        interceptor.cleanup();
        state.status = ContainerStatus::Stopped;
        state.pid = None;

        unsafe {
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            let _ = CloseHandle(job);
            let _ = CloseHandle(stdout_read);
        }

        // Close the write end of the stdin pipe — this signals EOF to the stdin
        // forwarding thread, allowing it to finish.
        if let Some((_r, stdin_write)) = &stdin_pipe {
            let _ = unsafe { CloseHandle(*stdin_write) };
        }

        let _ = stderr_thread.join();
        if let Some(t) = stdin_thread {
            let _ = t.join();
        }

        exit_code
    }

    fn stop(&self, state: &mut ContainerState) -> Result<()> {
        if let Some(pid) = state.pid {
            if crate::is_verbose() { eprintln!("[cell-guard] terminating PID {}", pid); }
            unsafe {
                let handle = OpenProcess(PROCESS_TERMINATE, false, pid);
                if let Ok(handle) = handle {
                    let _ = TerminateProcess(handle, 1);
                    let _ = CloseHandle(handle);
                }
            }
        }
        state.status = ContainerStatus::Stopped;
        state.pid = None;
        Ok(())
    }

    fn isolation_info(&self) -> IsolationInfo {
        IsolationInfo {
            platform: "Windows".into(),
            method: "Debug API + Job Objects + Syscall Interception".into(),
            filesystem: IsolationLevel::Intercepted,
            process: IsolationLevel::Intercepted,
            network: IsolationLevel::Intercepted,
            resources: IsolationLevel::Full,
        }
    }
}
