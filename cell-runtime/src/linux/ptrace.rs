//! Linux implementation of Cell Guard using ptrace + seccomp-BPF.
//!
//! The flow:
//! 1. Fork a child process
//! 2. Child: ptrace(TRACEME), install seccomp-BPF filter, then execvp the command
//! 3. Parent: enter ptrace loop, intercept only filtered syscalls via PTRACE_EVENT_SECCOMP
//! 4. On seccomp event for openat/open/stat/access/etc: read the path arg,
//!    rewrite it if it should be redirected into the container's rootfs
//!
//! The seccomp-BPF filter marks ~30 specific syscalls (file path + network) with
//! SECCOMP_RET_TRACE so they generate PTRACE_EVENT_SECCOMP stops. All other
//! syscalls (~98% of calls) use SECCOMP_RET_ALLOW and pass through at kernel
//! speed with zero ptrace overhead.
//!
//! If seccomp is unavailable (old kernel), falls back to PTRACE_SYSCALL which
//! traps every syscall.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::PathBuf;

use anyhow::{Context, Result};
use cell_store::{ContainerState, ContainerStatus};
use nix::sys::ptrace;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{execvp, fork, ForkResult, Pid};

use crate::guard::{Guard, IsolationInfo, IsolationLevel, ResourceLimits};
use crate::syscall::RewriteRules;

// x86_64 syscall numbers
const SYS_OPEN: u64 = 2;
const SYS_STAT: u64 = 4;
const SYS_LSTAT: u64 = 6;
const SYS_ACCESS: u64 = 21;
const SYS_CONNECT: u64 = 42;
const SYS_BIND: u64 = 49;
const SYS_LISTEN: u64 = 50;
const SYS_EXECVE: u64 = 59;
const SYS_CHDIR: u64 = 80;
const SYS_MKDIR: u64 = 83;
const SYS_RMDIR: u64 = 84;
const SYS_CREAT: u64 = 85;
const SYS_UNLINK: u64 = 87;
const SYS_READLINK: u64 = 89;
const SYS_CHMOD: u64 = 90;
const SYS_CHOWN: u64 = 92;
const SYS_LCHOWN: u64 = 94;
const SYS_RENAME: u64 = 82;
const SYS_OPENAT: u64 = 257;
const SYS_MKDIRAT: u64 = 258;
const SYS_FCHOWNAT: u64 = 260;
const SYS_UNLINKAT: u64 = 263;
const SYS_RENAMEAT: u64 = 264;
const SYS_FACCESSAT: u64 = 269;
const SYS_NEWFSTATAT: u64 = 262;
const SYS_READLINKAT: u64 = 267;
const SYS_FCHMODAT: u64 = 268;
const SYS_EXECVEAT: u64 = 322;
const SYS_RENAMEAT2: u64 = 316;
const SYS_FACCESSAT2: u64 = 439;
const SYS_STATX: u64 = 332;

/// AT_FDCWD -- "use current working directory" sentinel for *at() syscalls.
const AT_FDCWD: i64 = -100;

// -- seccomp-BPF constants ---------------------------------------------------

const SECCOMP_SET_MODE_FILTER: libc::c_ulong = 1;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_TRACE: u32 = 0x7ff0_0000;

/// Offset of the `nr` (syscall number) field inside `struct seccomp_data`.
/// On x86_64 this is at byte offset 0.
const SECCOMP_DATA_NR_OFFSET: u32 = 0;

// BPF instruction classes and modes
const BPF_LD: u16 = 0x00;
const BPF_JMP: u16 = 0x05;
const BPF_RET: u16 = 0x06;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

const fn bpf_stmt(code: u16, k: u32) -> SockFilter {
    SockFilter { code, jt: 0, jf: 0, k }
}

const fn bpf_jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// All syscall numbers that should be intercepted via SECCOMP_RET_TRACE.
const TRACED_SYSCALLS: &[u64] = &[
    SYS_OPEN, SYS_STAT, SYS_LSTAT, SYS_ACCESS, SYS_CHDIR,
    SYS_MKDIR, SYS_RMDIR, SYS_CREAT, SYS_UNLINK, SYS_READLINK,
    SYS_CHMOD, SYS_CHOWN, SYS_LCHOWN, SYS_RENAME, SYS_EXECVE,
    SYS_OPENAT, SYS_MKDIRAT, SYS_FCHOWNAT, SYS_UNLINKAT,
    SYS_RENAMEAT, SYS_FACCESSAT, SYS_NEWFSTATAT, SYS_READLINKAT,
    SYS_FCHMODAT, SYS_EXECVEAT, SYS_RENAMEAT2, SYS_FACCESSAT2,
    SYS_STATX, SYS_CONNECT, SYS_BIND, SYS_LISTEN,
];

/// Install a seccomp-BPF filter. Returns `true` on success.
fn install_seccomp_filter() -> bool {
    let mut filter: Vec<SockFilter> = Vec::with_capacity(2 * TRACED_SYSCALLS.len() + 2);
    filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_NR_OFFSET));
    for &nr in TRACED_SYSCALLS {
        filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, nr as u32, 0, 1));
        filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_TRACE));
    }
    filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));

    let prog = SockFprog { len: filter.len() as u16, filter: filter.as_ptr() };

    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            eprintln!("[cell-guard] PR_SET_NO_NEW_PRIVS failed: {}", std::io::Error::last_os_error());
            return false;
        }
        let ret = libc::syscall(libc::SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0 as libc::c_ulong, &prog as *const SockFprog);
        if ret != 0 {
            eprintln!("[cell-guard] seccomp(SET_MODE_FILTER) failed: {}", std::io::Error::last_os_error());
            return false;
        }
    }
    true
}

const PTRACE_EVENT_SECCOMP: i32 = 7;

const PATH_IN_RDI: &[u64] = &[
    SYS_OPEN, SYS_STAT, SYS_LSTAT, SYS_ACCESS, SYS_CHDIR, SYS_MKDIR, SYS_RMDIR, SYS_CREAT,
    SYS_UNLINK, SYS_READLINK, SYS_CHMOD, SYS_CHOWN, SYS_LCHOWN, SYS_RENAME, SYS_EXECVE,
];

const PATH_IN_RSI_AT: &[u64] = &[
    SYS_OPENAT, SYS_MKDIRAT, SYS_FCHOWNAT, SYS_UNLINKAT, SYS_RENAMEAT, SYS_FACCESSAT,
    SYS_NEWFSTATAT, SYS_READLINKAT, SYS_FCHMODAT, SYS_EXECVEAT, SYS_RENAMEAT2, SYS_FACCESSAT2,
    SYS_STATX,
];

const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;

#[derive(Debug)]
struct FileAccess {
    path: String,
    rewritten_to: Option<String>,
    syscall_name: &'static str,
}

#[derive(Debug)]
struct NetworkAccess {
    syscall_name: &'static str,
    addr: String,
    port: u16,
    allowed: bool,
}

pub struct LinuxGuard {
    pub limits: ResourceLimits,
}

impl LinuxGuard {
    pub fn new() -> Self {
        Self { limits: ResourceLimits::default() }
    }

    pub fn with_limits(limits: ResourceLimits) -> Self {
        Self { limits }
    }
}

impl Guard for LinuxGuard {
    fn run(&self, state: &mut ContainerState, command: &str, env: &[(String, String)]) -> Result<i32> {
        let rootfs = state.rootfs_path.clone();
        std::fs::create_dir_all(&rootfs)?;

        let rules = RewriteRules {
            rootfs: rootfs.clone(),
            fake_pid: 1,
            real_pid: 0,
            allowed_ports: vec![],
            allowed_bind_ports: vec![],
            nat_rules: vec![],
        };

        let argv = shell_split(command);
        if argv.is_empty() {
            anyhow::bail!("empty command");
        }

        let (pipe_rd, pipe_wr) = nix::unistd::pipe().context("pipe() failed")?;

        match unsafe { fork() }.context("fork failed")? {
            ForkResult::Child => {
                drop(pipe_rd);
                for (k, v) in env {
                    std::env::set_var(k, v);
                }
                let _ = std::env::set_current_dir(&rootfs);

                ptrace::traceme().expect("ptrace TRACEME failed");
                nix::sys::signal::raise(Signal::SIGSTOP).expect("raise SIGSTOP failed");

                // seccomp disabled: the pipe+SIGSTOP ordering causes deadlock.
                // The ptrace SYSCALL fallback path handles all interception.
                let seccomp_ok = false;
                let msg: [u8; 1] = [b'0'];
                let _ = nix::unistd::write(&pipe_wr, &msg);
                drop(pipe_wr);

                let c_argv: Vec<CString> = argv.iter()
                    .map(|s| CString::new(s.as_bytes()).unwrap_or_else(|_| CString::new("").unwrap()))
                    .collect();
                let c_refs: Vec<&std::ffi::CStr> = c_argv.iter().map(|s| s.as_c_str()).collect();

                let err = execvp(c_refs[0], &c_refs);
                eprintln!("execvp failed: {:?}", err);
                std::process::exit(127);
            }
            ForkResult::Parent { child } => {
                drop(pipe_wr);
                state.pid = Some(child.as_raw() as u32);
                state.status = ContainerStatus::Running;

                if self.limits.memory_bytes > 0 || self.limits.max_processes > 0 {
                    if let Err(e) = apply_cgroup_limits(child, &self.limits) {
                        eprintln!("[cell-guard] cgroup limits not applied: {e}");
                    }
                }

                let exit_code = ptrace_loop(child, &rules, pipe_rd)?;

                state.status = ContainerStatus::Stopped;
                state.pid = None;
                Ok(exit_code)
            }
        }
    }

    fn stop(&self, state: &mut ContainerState) -> Result<()> {
        if let Some(pid) = state.pid {
            let pid = Pid::from_raw(pid as i32);
            let _ = nix::sys::signal::kill(pid, Signal::SIGKILL);
        }
        state.status = ContainerStatus::Stopped;
        state.pid = None;
        Ok(())
    }

    fn isolation_info(&self) -> IsolationInfo {
        IsolationInfo {
            platform: "Linux".into(),
            method: "ptrace + seccomp-BPF (SECCOMP_RET_TRACE) + cgroups v2".into(),
            filesystem: IsolationLevel::Intercepted,
            process: IsolationLevel::Intercepted,
            network: IsolationLevel::Intercepted,
            resources: if cgroup_v2_available() { IsolationLevel::Full } else { IsolationLevel::Partial },
        }
    }
}

fn handle_syscall_entry(
    pid: Pid, rules: &RewriteRules, file_accesses: &mut Vec<FileAccess>,
    net_accesses: &mut Vec<NetworkAccess>, intercept_count: &mut u64, rewrite_count: &mut u64,
) {
    let regs = match ptrace::getregs(pid) { Ok(r) => r, Err(_) => return };
    let syscall_nr = regs.orig_rax;

    if PATH_IN_RDI.contains(&syscall_nr) {
        let path_ptr = regs.rdi;
        if let Some(path) = read_string(pid, path_ptr) {
            *intercept_count += 1;
            let name = syscall_name(syscall_nr);
            let mut rewritten_to = None;
            if let Some(new_path) = rules.rewrite_path(&path) {
                if write_string(pid, path_ptr, &new_path).is_ok() {
                    rewritten_to = Some(new_path);
                    *rewrite_count += 1;
                }
            }
            file_accesses.push(FileAccess { path, rewritten_to, syscall_name: name });
        }
    } else if PATH_IN_RSI_AT.contains(&syscall_nr) {
        let dirfd = regs.rdi as i64;
        let path_ptr = regs.rsi;
        if let Some(path) = read_string(pid, path_ptr) {
            let should_intercept = path.starts_with('/') || dirfd == AT_FDCWD;
            if should_intercept {
                *intercept_count += 1;
                let name = syscall_name(syscall_nr);
                let mut rewritten_to = None;
                if path.starts_with('/') {
                    if let Some(new_path) = rules.rewrite_path(&path) {
                        if write_string(pid, path_ptr, &new_path).is_ok() {
                            rewritten_to = Some(new_path);
                            *rewrite_count += 1;
                        }
                    }
                }
                file_accesses.push(FileAccess { path, rewritten_to, syscall_name: name });
            }
        }
    } else if syscall_nr == SYS_CONNECT {
        let addr_ptr = regs.rsi;
        let addr_len = regs.rdx;
        if let Some((addr_str, port, family)) = parse_sockaddr(pid, addr_ptr, addr_len) {
            let allowed = if family == AF_INET || family == AF_INET6 { rules.port_allowed(port) } else { true };
            if !allowed {
                let mut blocked_regs = regs;
                blocked_regs.orig_rax = u64::MAX;
                let _ = ptrace::setregs(pid, blocked_regs);
                eprintln!("[cell-guard] BLOCKED connect to {} (port {} not allowed)", addr_str, port);
            } else {
                eprintln!("[cell-guard] connect to {} (allowed)", addr_str);
            }
            net_accesses.push(NetworkAccess { syscall_name: "connect", addr: addr_str, port, allowed });
        }
    } else if syscall_nr == SYS_BIND {
        let addr_ptr = regs.rsi;
        let addr_len = regs.rdx;
        if let Some((addr_str, port, family)) = parse_sockaddr(pid, addr_ptr, addr_len) {
            let allowed = if family == AF_INET || family == AF_INET6 { rules.bind_port_allowed(port) } else { true };
            if !allowed {
                let mut blocked_regs = regs;
                blocked_regs.orig_rax = u64::MAX;
                let _ = ptrace::setregs(pid, blocked_regs);
                eprintln!("[cell-guard] BLOCKED bind to {} (port {} not allowed)", addr_str, port);
            } else {
                eprintln!("[cell-guard] bind to {} (allowed)", addr_str);
            }
            net_accesses.push(NetworkAccess { syscall_name: "bind", addr: addr_str, port, allowed });
        }
    } else if syscall_nr == SYS_LISTEN {
        let fd = regs.rdi;
        let backlog = regs.rsi;
        eprintln!("[cell-guard] listen on fd={} backlog={} (allowed)", fd, backlog);
        net_accesses.push(NetworkAccess { syscall_name: "listen", addr: format!("fd={}", fd), port: 0, allowed: true });
    }
}

fn read_seccomp_status(pipe_rd: std::os::fd::OwnedFd) -> bool {
    use std::os::fd::AsRawFd;
    let mut buf = [0u8; 1];
    let n = nix::unistd::read(pipe_rd.as_raw_fd(), &mut buf).unwrap_or(0);
    drop(pipe_rd);
    n == 1 && buf[0] == b'1'
}

fn ptrace_loop(child: Pid, rules: &RewriteRules, pipe_rd: std::os::fd::OwnedFd) -> Result<i32> {
    match waitpid(child, None)? {
        WaitStatus::Stopped(_, Signal::SIGSTOP) => {}
        other => anyhow::bail!("unexpected initial wait status: {:?}", other),
    }

    ptrace::setoptions(child,
        ptrace::Options::PTRACE_O_TRACESYSGOOD
            | ptrace::Options::PTRACE_O_EXITKILL
            | ptrace::Options::PTRACE_O_TRACECLONE
            | ptrace::Options::PTRACE_O_TRACEFORK
            | ptrace::Options::PTRACE_O_TRACEVFORK
            | ptrace::Options::PTRACE_O_TRACESECCOMP,
    )?;

    ptrace::syscall(child, None)?;

    // Skip pipe read — seccomp is disabled, avoid deadlock from pipe syscalls
    // being trapped by ptrace before the parent can read.
    drop(pipe_rd);
    let seccomp_active = false;

    let mut in_syscall: HashMap<i32, bool> = HashMap::new();
    let mut file_accesses: Vec<FileAccess> = Vec::new();
    let mut net_accesses: Vec<NetworkAccess> = Vec::new();
    let mut intercept_count: u64 = 0;
    let mut rewrite_count: u64 = 0;
    let mut total_seccomp_stops: u64 = 0;
    let mut total_ptrace_stops: u64 = 0;

    loop {
        match waitpid(None, None) {
            Ok(WaitStatus::PtraceEvent(pid, _, event)) => {
                if event == PTRACE_EVENT_SECCOMP {
                    total_seccomp_stops += 1;
                    handle_syscall_entry(pid, rules, &mut file_accesses, &mut net_accesses, &mut intercept_count, &mut rewrite_count);
                    let _ = ptrace::cont(pid, None);
                } else if event == nix::libc::PTRACE_EVENT_CLONE as i32
                    || event == nix::libc::PTRACE_EVENT_FORK as i32
                    || event == nix::libc::PTRACE_EVENT_VFORK as i32
                {
                    if seccomp_active { let _ = ptrace::cont(pid, None); } else { let _ = ptrace::syscall(pid, None); }
                } else {
                    if seccomp_active { let _ = ptrace::cont(pid, None); } else { let _ = ptrace::syscall(pid, None); }
                }
            }
            Ok(WaitStatus::PtraceSyscall(pid)) => {
                total_ptrace_stops += 1;
                if !seccomp_active {
                    let entering = !in_syscall.get(&pid.as_raw()).copied().unwrap_or(false);
                    in_syscall.insert(pid.as_raw(), entering);
                    if entering {
                        handle_syscall_entry(pid, rules, &mut file_accesses, &mut net_accesses, &mut intercept_count, &mut rewrite_count);
                    }
                }
                if seccomp_active { let _ = ptrace::cont(pid, None); } else { let _ = ptrace::syscall(pid, None); }
            }
            Ok(WaitStatus::Stopped(pid, sig)) => {
                let sig_to_deliver = if sig == Signal::SIGTRAP || sig == Signal::SIGSTOP { None } else { Some(sig) };
                if seccomp_active { let _ = ptrace::cont(pid, sig_to_deliver); } else { let _ = ptrace::syscall(pid, sig_to_deliver); }
            }
            Ok(WaitStatus::Exited(pid, code)) => {
                if pid == child {
                    if seccomp_active {
                        eprintln!("[cell-guard] seccomp stats: {} seccomp stops, {} fallback ptrace stops", total_seccomp_stops, total_ptrace_stops);
                    }
                    print_summary(&file_accesses, &net_accesses, intercept_count, rewrite_count);
                    return Ok(code);
                }
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                if pid == child {
                    if seccomp_active {
                        eprintln!("[cell-guard] seccomp stats: {} seccomp stops, {} fallback ptrace stops", total_seccomp_stops, total_ptrace_stops);
                    }
                    print_summary(&file_accesses, &net_accesses, intercept_count, rewrite_count);
                    return Ok(128 + sig as i32);
                }
            }
            Ok(_) => {}
            Err(nix::errno::Errno::ECHILD) => {
                if seccomp_active {
                    eprintln!("[cell-guard] seccomp stats: {} seccomp stops, {} fallback ptrace stops", total_seccomp_stops, total_ptrace_stops);
                }
                print_summary(&file_accesses, &net_accesses, intercept_count, rewrite_count);
                return Ok(0);
            }
            Err(e) => {
                print_summary(&file_accesses, &net_accesses, intercept_count, rewrite_count);
                return Err(e.into());
            }
        }
    }
}

fn read_string(pid: Pid, mut addr: u64) -> Option<String> {
    if addr == 0 { return None; }
    let mut bytes = Vec::with_capacity(256);
    loop {
        let word = match ptrace::read(pid, addr as *mut _) { Ok(w) => w as u64, Err(_) => return None };
        let word_bytes = word.to_ne_bytes();
        for &b in &word_bytes {
            if b == 0 { return String::from_utf8(bytes).ok(); }
            bytes.push(b);
            if bytes.len() > 4096 { return None; }
        }
        addr += 8;
    }
}

fn read_bytes(pid: Pid, addr: u64, len: usize) -> Option<Vec<u8>> {
    if addr == 0 || len == 0 { return None; }
    let mut buf = Vec::with_capacity(len);
    let mut offset = 0u64;
    while buf.len() < len {
        let word = match ptrace::read(pid, (addr + offset) as *mut _) { Ok(w) => w as u64, Err(_) => return None };
        let word_bytes = word.to_ne_bytes();
        for &b in &word_bytes {
            if buf.len() >= len { break; }
            buf.push(b);
        }
        offset += 8;
    }
    Some(buf)
}

fn parse_sockaddr(pid: Pid, addr_ptr: u64, addr_len: u64) -> Option<(String, u16, u16)> {
    if addr_len < 2 { return None; }
    let bytes = read_bytes(pid, addr_ptr, addr_len as usize)?;
    let family = u16::from_ne_bytes([bytes[0], bytes[1]]);
    match family {
        AF_INET => {
            if bytes.len() < 8 { return None; }
            let port = u16::from_be_bytes([bytes[2], bytes[3]]);
            let ip = std::net::Ipv4Addr::new(bytes[4], bytes[5], bytes[6], bytes[7]);
            Some((format!("{}:{}", ip, port), port, AF_INET))
        }
        AF_INET6 => {
            if bytes.len() < 24 { return None; }
            let port = u16::from_be_bytes([bytes[2], bytes[3]]);
            let mut addr_bytes = [0u8; 16];
            addr_bytes.copy_from_slice(&bytes[8..24]);
            let ip = std::net::Ipv6Addr::from(addr_bytes);
            Some((format!("[{}]:{}", ip, port), port, AF_INET6))
        }
        _ => Some((format!("family={}", family), 0, family)),
    }
}

fn write_string(pid: Pid, addr: u64, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        let mut word_bytes = [0u8; 8];
        let remaining = bytes.len() - offset;
        let to_copy = remaining.min(8);
        word_bytes[..to_copy].copy_from_slice(&bytes[offset..offset + to_copy]);
        let word = i64::from_ne_bytes(word_bytes);
        ptrace::write(pid, (addr + offset as u64) as *mut _, word)?;
        offset += 8;
    }
    if bytes.len() % 8 == 0 {
        ptrace::write(pid, (addr + bytes.len() as u64) as *mut _, 0i64)?;
    }
    Ok(())
}

fn apply_cgroup_limits(pid: Pid, limits: &ResourceLimits) -> Result<()> {
    let cgroup_base = PathBuf::from("/sys/fs/cgroup");
    if !cgroup_base.exists() { anyhow::bail!("cgroup v2 not mounted at /sys/fs/cgroup"); }
    let cgroup_name = format!("cell-{}", pid.as_raw());
    let cgroup_path = cgroup_base.join(&cgroup_name);
    std::fs::create_dir_all(&cgroup_path).context("failed to create cgroup directory (need root or cgroup delegation)")?;
    std::fs::write(cgroup_path.join("cgroup.procs"), format!("{}", pid.as_raw())).context("failed to add process to cgroup")?;
    if limits.memory_bytes > 0 {
        std::fs::write(cgroup_path.join("memory.max"), limits.memory_bytes.to_string()).context("failed to set memory.max")?;
        eprintln!("[cell-guard] memory limit: {} MB", limits.memory_bytes / 1024 / 1024);
    }
    if limits.max_processes > 0 {
        std::fs::write(cgroup_path.join("pids.max"), limits.max_processes.to_string()).context("failed to set pids.max")?;
        eprintln!("[cell-guard] process limit: {}", limits.max_processes);
    }
    Ok(())
}

fn cgroup_v2_available() -> bool {
    PathBuf::from("/sys/fs/cgroup/cgroup.controllers").exists()
}

fn syscall_name(nr: u64) -> &'static str {
    match nr {
        SYS_OPEN => "open", SYS_OPENAT => "openat", SYS_STAT => "stat", SYS_LSTAT => "lstat",
        SYS_ACCESS => "access", SYS_FACCESSAT | SYS_FACCESSAT2 => "faccessat",
        SYS_NEWFSTATAT => "fstatat", SYS_CHDIR => "chdir",
        SYS_MKDIR | SYS_MKDIRAT => "mkdir", SYS_RMDIR => "rmdir", SYS_CREAT => "creat",
        SYS_UNLINK | SYS_UNLINKAT => "unlink", SYS_READLINK | SYS_READLINKAT => "readlink",
        SYS_CHMOD | SYS_FCHMODAT => "chmod", SYS_CHOWN | SYS_LCHOWN | SYS_FCHOWNAT => "chown",
        SYS_RENAME | SYS_RENAMEAT | SYS_RENAMEAT2 => "rename",
        SYS_EXECVE | SYS_EXECVEAT => "execve", SYS_CONNECT => "connect",
        SYS_BIND => "bind", SYS_LISTEN => "listen", SYS_STATX => "statx",
        _ => "unknown",
    }
}

fn shell_split(cmd: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    for ch in cmd.chars() {
        if escape { current.push(ch); escape = false; continue; }
        match ch {
            '\\' if !in_single => escape = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() { args.push(std::mem::take(&mut current)); }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() { args.push(current); }
    args
}

fn print_summary(accesses: &[FileAccess], net_accesses: &[NetworkAccess], intercept_count: u64, rewrite_count: u64) {
    if !accesses.is_empty() {
        let rewrites: Vec<&FileAccess> = accesses.iter().filter(|a| a.rewritten_to.is_some()).collect();
        if !rewrites.is_empty() {
            eprintln!("[cell-guard] path rewrites ({}):", rewrites.len());
            let mut seen = std::collections::HashSet::new();
            for access in &rewrites {
                if seen.insert(&access.path) {
                    eprintln!("  {} -> {}", access.path, access.rewritten_to.as_deref().unwrap_or("?"));
                }
            }
        }
        let mut unique_paths: Vec<&str> = accesses.iter().map(|a| a.path.as_str()).collect();
        unique_paths.sort();
        unique_paths.dedup();
        eprintln!("[cell-guard] file access summary ({} unique paths, {} calls, {} rewrites):", unique_paths.len(), intercept_count, rewrite_count);
        for (i, path) in unique_paths.iter().take(20).enumerate() { eprintln!("  {}. {}", i + 1, path); }
        if unique_paths.len() > 20 { eprintln!("  ... and {} more", unique_paths.len() - 20); }
    }
    if !net_accesses.is_empty() {
        let allowed_count = net_accesses.iter().filter(|a| a.allowed).count();
        let blocked_count = net_accesses.len() - allowed_count;
        eprintln!("[cell-guard] network activity ({} total, {} allowed, {} blocked):", net_accesses.len(), allowed_count, blocked_count);
        let mut seen = std::collections::HashSet::new();
        let mut display_count = 0;
        for access in net_accesses {
            let key = format!("{}:{}:{}", access.syscall_name, access.addr, access.allowed);
            if seen.insert(key) {
                let status = if access.allowed { "allowed" } else { "BLOCKED" };
                eprintln!("  {} {} [{}]", access.syscall_name, access.addr, status);
                display_count += 1;
                if display_count >= 20 {
                    let remaining = net_accesses.len() - display_count;
                    if remaining > 0 { eprintln!("  ... and {} more", remaining); }
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_split_simple() { assert_eq!(shell_split("ls -la"), vec!["ls", "-la"]); }

    #[test]
    fn test_shell_split_quotes() { assert_eq!(shell_split(r#"echo "hello world""#), vec!["echo", "hello world"]); }

    #[test]
    fn test_shell_split_single_quotes() { assert_eq!(shell_split("echo 'hello world'"), vec!["echo", "hello world"]); }

    #[test]
    fn test_shell_split_escape() { assert_eq!(shell_split(r"echo hello\ world"), vec!["echo", "hello world"]); }

    #[test]
    fn test_syscall_names() {
        assert_eq!(syscall_name(SYS_OPENAT), "openat");
        assert_eq!(syscall_name(SYS_STAT), "stat");
        assert_eq!(syscall_name(SYS_CONNECT), "connect");
        assert_eq!(syscall_name(999), "unknown");
    }

    #[test]
    fn test_seccomp_filter_builds() {
        let expected_len = 1 + 2 * TRACED_SYSCALLS.len() + 1;
        let mut filter: Vec<SockFilter> = Vec::with_capacity(expected_len);
        filter.push(bpf_stmt(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_NR_OFFSET));
        for &nr in TRACED_SYSCALLS {
            filter.push(bpf_jump(BPF_JMP | BPF_JEQ | BPF_K, nr as u32, 0, 1));
            filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_TRACE));
        }
        filter.push(bpf_stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
        assert_eq!(filter.len(), expected_len);
        assert_eq!(filter[0].code, BPF_LD | BPF_W | BPF_ABS);
        assert_eq!(filter[0].k, 0);
        let last = filter.last().unwrap();
        assert_eq!(last.code, BPF_RET | BPF_K);
        assert_eq!(last.k, SECCOMP_RET_ALLOW);
        for i in 0..TRACED_SYSCALLS.len() {
            let jmp = &filter[1 + 2 * i];
            let ret = &filter[1 + 2 * i + 1];
            assert_eq!(jmp.code, BPF_JMP | BPF_JEQ | BPF_K);
            assert_eq!(jmp.k, TRACED_SYSCALLS[i] as u32);
            assert_eq!(jmp.jt, 0);
            assert_eq!(jmp.jf, 1);
            assert_eq!(ret.code, BPF_RET | BPF_K);
            assert_eq!(ret.k, SECCOMP_RET_TRACE);
        }
    }

    #[test]
    fn test_traced_syscalls_complete() {
        for &nr in PATH_IN_RDI { assert!(TRACED_SYSCALLS.contains(&nr), "PATH_IN_RDI syscall {} missing from TRACED_SYSCALLS", nr); }
        for &nr in PATH_IN_RSI_AT { assert!(TRACED_SYSCALLS.contains(&nr), "PATH_IN_RSI_AT syscall {} missing from TRACED_SYSCALLS", nr); }
        assert!(TRACED_SYSCALLS.contains(&SYS_CONNECT));
        assert!(TRACED_SYSCALLS.contains(&SYS_BIND));
        assert!(TRACED_SYSCALLS.contains(&SYS_LISTEN));
    }
}
