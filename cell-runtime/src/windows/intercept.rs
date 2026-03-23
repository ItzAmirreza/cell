//! Syscall interception via INT3 breakpoints on NT API functions.
//!
//! This module is the heart of Cell Guard on Windows. It:
//! 1. Finds NtCreateFile/NtOpenFile addresses in the target process's ntdll.dll
//! 2. Sets INT3 (0xCC) breakpoints on those functions
//! 3. When a breakpoint hits, reads the file path from the OBJECT_ATTRIBUTES argument
//! 4. Logs all file access for visibility
//! 5. (Future) Rewrites paths to redirect into the container's rootfs

use std::collections::HashMap;
use std::mem;
use std::path::PathBuf;

use anyhow::{Context, Result};
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::Debug::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Threading::*;

// ── Raw FFI for thread context (not exposed by windows 0.58 feature flags) ──

// x64 CONTEXT flags
const CONTEXT_AMD64: u32 = 0x00100000;
const CONTEXT_CONTROL_FLAG: u32 = CONTEXT_AMD64 | 0x01;
const CONTEXT_INTEGER_FLAG: u32 = CONTEXT_AMD64 | 0x02;
const CONTEXT_FULL_FLAG: u32 = CONTEXT_CONTROL_FLAG | CONTEXT_INTEGER_FLAG | (CONTEXT_AMD64 | 0x08);

/// x64 CONTEXT structure (simplified — we only need the fields we read/write).
/// Full struct is 1232 bytes. We allocate the full size but only access specific fields.
#[repr(C, align(16))]
struct Amd64Context {
    // Offset 0x00: Control flags
    p1_home: u64,
    p2_home: u64,
    p3_home: u64,
    p4_home: u64,
    p5_home: u64,
    p6_home: u64,
    // Offset 0x30
    context_flags: u32,
    mx_csr: u32,
    // Offset 0x38: Segment registers
    seg_cs: u16,
    seg_ds: u16,
    seg_es: u16,
    seg_fs: u16,
    seg_gs: u16,
    seg_ss: u16,
    eflags: u32,
    // Offset 0x48: Debug registers
    dr0: u64,
    dr1: u64,
    dr2: u64,
    dr3: u64,
    dr6: u64,
    dr7: u64,
    // Offset 0x78: Integer registers
    rax: u64,
    rcx: u64,
    rdx: u64,
    rbx: u64,
    rsp: u64,
    rbp: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    // Offset 0xF8: Program counter
    rip: u64,
    // Remaining fields (FP/SSE state) — we don't need them but must allocate space
    _flt_save: [u8; 512],
    _vector_register: [u8; 416],
    _debug_control: u64,
    _last_branch_to_rip: u64,
    _last_branch_from_rip: u64,
    _last_exception_to_rip: u64,
    _last_exception_from_rip: u64,
}

extern "system" {
    fn GetThreadContext(h_thread: HANDLE, lp_context: *mut Amd64Context) -> BOOL;
    fn SetThreadContext(h_thread: HANDLE, lp_context: *const Amd64Context) -> BOOL;
    fn FlushInstructionCache(h_process: HANDLE, lp_base: *const std::ffi::c_void, dw_size: usize) -> BOOL;
}

// ── Interceptor types ───────────────────────────────────────────────────────

/// A breakpoint set in the target process.
/// Which calling convention the breakpoint uses.
#[derive(Debug, Clone, Copy)]
enum BpKind {
    /// File syscall: OBJECT_ATTRIBUTES in RCX (1st arg)
    FileRcx,
    /// File syscall: OBJECT_ATTRIBUTES in R8 (3rd arg)
    FileR8,
    /// Winsock connect: sockaddr* in RDX (2nd arg), namelen in R8 (3rd arg)
    NetConnect,
    /// Winsock bind: sockaddr* in RDX (2nd arg), namelen in R8 (3rd arg)
    NetBind,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Breakpoint {
    address: u64,
    original_byte: u8,
    name: String,
    /// What kind of syscall this breakpoint intercepts.
    kind: BpKind,
}

/// Tracks file access by the contained process.
#[derive(Debug, Clone)]
pub struct FileAccess {
    pub path: String,
    pub rewritten_to: Option<String>,
}

/// Tracks network connection attempts.
#[derive(Debug, Clone)]
pub struct NetAccess {
    pub addr: String,
    pub port: u16,
    pub blocked: bool,
}

/// The syscall interceptor. Manages breakpoints and path rewriting.
pub struct Interceptor {
    process: HANDLE,
    breakpoints: HashMap<u64, Breakpoint>,
    /// Container rootfs path in NT format (e.g. `\??\C:\Users\...\.cell\containers\abc\rootfs`)
    rootfs_nt: String,
    /// Container rootfs path for display.
    #[allow(dead_code)]
    rootfs_display: String,
    rewrite_enabled: bool,
    pub file_accesses: Vec<FileAccess>,
    pub net_accesses: Vec<NetAccess>,
    /// Allowed network ports (empty = allow all).
    allowed_ports: Vec<u16>,
    /// Port mappings: (host_port, container_port). When the process binds to container_port,
    /// rewrite the sockaddr to use host_port instead.
    port_mappings: Vec<(u16, u16)>,
    /// Volume mounts: (container_path_nt, host_volume_path_nt).
    /// container_path_nt is the NT-format path the container sees (e.g. `\??\C:\app\data`).
    /// host_volume_path_nt is the NT-format path of the actual volume on the host.
    volume_mounts: Vec<(String, String)>,
    pending_single_step: Option<(u32, u64)>,
    pub intercept_count: u64,
    pub rewrite_count: u64,
}

/// UNICODE_STRING as laid out in target process memory (x64).
#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    _padding: u32,
    buffer: u64,
}

/// OBJECT_ATTRIBUTES as laid out in target process memory (x64).
#[repr(C)]
#[derive(Clone, Copy)]
struct ObjectAttributes {
    length: u32,
    _padding1: u32,
    root_directory: u64,
    object_name: u64,
    attributes: u32,
    _padding2: u32,
    security_descriptor: u64,
    security_quality_of_service: u64,
}

/// Info needed to read and optionally rewrite a file path.
struct NtFileInfo {
    path: String,
    /// Address of the UNICODE_STRING struct in the target process.
    ustr_addr: u64,
    /// The UNICODE_STRING as read from the target.
    ustr: UnicodeString,
    /// Maximum buffer size available for rewriting.
    max_len: u16,
}

// ── Helper: read/write thread context ───────────────────────────────────────

fn get_context(thread_id: u32, flags: u32) -> Result<(HANDLE, Amd64Context)> {
    let handle = unsafe {
        OpenThread(THREAD_GET_CONTEXT | THREAD_SET_CONTEXT | THREAD_SUSPEND_RESUME, false, thread_id)
    }
    .context("OpenThread failed")?;

    let mut ctx: Amd64Context = unsafe { mem::zeroed() };
    ctx.context_flags = flags;

    let ok = unsafe { GetThreadContext(handle, &mut ctx) };
    if !ok.as_bool() {
        let _ = unsafe { CloseHandle(handle) };
        anyhow::bail!("GetThreadContext failed (err: {})", std::io::Error::last_os_error());
    }

    Ok((handle, ctx))
}

fn set_context(handle: HANDLE, ctx: &Amd64Context) -> Result<()> {
    let ok = unsafe { SetThreadContext(handle, ctx) };
    if !ok.as_bool() {
        anyhow::bail!("SetThreadContext failed (err: {})", std::io::Error::last_os_error());
    }
    Ok(())
}

fn flush_icache(process: HANDLE, address: u64, size: usize) {
    let _ = unsafe { FlushInstructionCache(process, address as *const _, size) };
}

// ── Interceptor implementation ──────────────────────────────────────────────

impl Interceptor {
    /// Create a new interceptor.
    ///
    /// - `port_mappings`: list of `(host_port, container_port)`. When the container calls
    ///   `bind()` on `container_port`, the interceptor rewrites the sockaddr to `host_port`.
    /// - `volume_mounts`: list of `(container_path, host_volume_path)` in NT format
    ///   (`\??\C:\...`). File paths under `container_path` are redirected to the
    ///   corresponding location under `host_volume_path` instead of the rootfs.
    pub fn new(
        process: HANDLE,
        rootfs: PathBuf,
        rewrite_enabled: bool,
        port_mappings: Vec<(u16, u16)>,
        volume_mounts: Vec<(String, String)>,
    ) -> Self {
        // Convert rootfs to NT path format: \??\C:\path\to\rootfs
        let rootfs_str = rootfs.to_string_lossy().to_string();
        let rootfs_nt = format!("\\??\\{}", rootfs_str);
        Self {
            process,
            breakpoints: HashMap::new(),
            rootfs_nt,
            rootfs_display: rootfs_str,
            rewrite_enabled,
            file_accesses: Vec::new(),
            net_accesses: Vec::new(),
            allowed_ports: Vec::new(), // empty = allow all
            port_mappings,
            volume_mounts,
            pending_single_step: None,
            intercept_count: 0,
            rewrite_count: 0,
        }
    }

    /// Set up breakpoints on key NT API functions.
    pub fn setup_breakpoints(&mut self) -> Result<()> {
        let ntdll = unsafe { GetModuleHandleA(windows::core::s!("ntdll.dll")) }
            .context("ntdll.dll not found")?;

        // ── Filesystem syscalls ──
        if let Some(addr) = unsafe { GetProcAddress(ntdll, windows::core::s!("NtCreateFile")) } {
            self.set_breakpoint(addr as u64, "NtCreateFile".into(), BpKind::FileR8)?;
        }
        if let Some(addr) = unsafe { GetProcAddress(ntdll, windows::core::s!("NtOpenFile")) } {
            self.set_breakpoint(addr as u64, "NtOpenFile".into(), BpKind::FileR8)?;
        }
        if let Some(addr) = unsafe { GetProcAddress(ntdll, windows::core::s!("NtQueryAttributesFile")) } {
            self.set_breakpoint(addr as u64, "NtQueryAttributesFile".into(), BpKind::FileRcx)?;
        }
        if let Some(addr) = unsafe { GetProcAddress(ntdll, windows::core::s!("NtQueryFullAttributesFile")) } {
            self.set_breakpoint(addr as u64, "NtQueryFullAttributesFile".into(), BpKind::FileRcx)?;
        }
        if let Some(addr) = unsafe { GetProcAddress(ntdll, windows::core::s!("NtDeleteFile")) } {
            self.set_breakpoint(addr as u64, "NtDeleteFile".into(), BpKind::FileRcx)?;
        }

        // ── Network syscalls (Winsock) ──
        // Load ws2_32.dll into our own process so we can look up function addresses.
        // It maps at the same base in the child process (same ASLR session).
        let ws2 = unsafe { LoadLibraryA(windows::core::s!("ws2_32.dll")) };
        if let Ok(ws2) = ws2 {
            if let Some(addr) = unsafe { GetProcAddress(ws2, windows::core::s!("connect")) } {
                let _ = self.set_breakpoint(addr as u64, "connect".into(), BpKind::NetConnect);
            }
            if let Some(addr) = unsafe { GetProcAddress(ws2, windows::core::s!("bind")) } {
                let _ = self.set_breakpoint(addr as u64, "bind".into(), BpKind::NetBind);
            }
        }

        if crate::is_verbose() {
            let fs_count = self.breakpoints.values().filter(|b| matches!(b.kind, BpKind::FileR8 | BpKind::FileRcx)).count();
            let net_count = self.breakpoints.values().filter(|b| matches!(b.kind, BpKind::NetConnect | BpKind::NetBind)).count();
            eprintln!(
                "[cell-guard] {} breakpoints set ({} filesystem, {} network)",
                self.breakpoints.len(), fs_count, net_count
            );
        }
        Ok(())
    }

    /// Write INT3 at the given address in the target process.
    fn set_breakpoint(&mut self, address: u64, name: String, kind: BpKind) -> Result<()> {
        let mut original_byte: u8 = 0;
        let mut n: usize = 0;

        unsafe {
            ReadProcessMemory(
                self.process,
                address as *const _,
                &mut original_byte as *mut _ as *mut _,
                1,
                Some(&mut n),
            )
            .context("ReadProcessMemory failed")?;
        }

        let int3: u8 = 0xCC;
        unsafe {
            WriteProcessMemory(
                self.process,
                address as *const _,
                &int3 as *const _ as *const _,
                1,
                Some(&mut n),
            )
            .context("WriteProcessMemory (INT3) failed")?;
        }
        flush_icache(self.process, address, 1);

        self.breakpoints.insert(address, Breakpoint { address, original_byte, name, kind });
        Ok(())
    }

    /// Restore original byte, re-arm after single-step.
    fn restore_byte(&self, bp: &Breakpoint) -> Result<()> {
        let mut n: usize = 0;
        unsafe {
            WriteProcessMemory(
                self.process,
                bp.address as *const _,
                &bp.original_byte as *const _ as *const _,
                1,
                Some(&mut n),
            )
            .context("restore byte failed")?;
        }
        flush_icache(self.process, bp.address, 1);
        Ok(())
    }

    fn rearm_breakpoint(&self, address: u64) {
        let int3: u8 = 0xCC;
        let mut n: usize = 0;
        unsafe {
            let _ = WriteProcessMemory(
                self.process,
                address as *const _,
                &int3 as *const _ as *const _,
                1,
                Some(&mut n),
            );
        }
        flush_icache(self.process, address, 1);
    }

    /// Handle EXCEPTION_BREAKPOINT. Returns true if it was one of ours.
    pub fn handle_breakpoint(
        &mut self,
        thread_id: u32,
        exception_address: u64,
    ) -> Result<bool> {
        let bp = match self.breakpoints.get(&exception_address) {
            Some(bp) => bp.clone(),
            None => return Ok(false),
        };

        self.intercept_count += 1;

        match bp.kind {
            BpKind::FileR8 | BpKind::FileRcx => {
                let file_arg = match bp.kind {
                    BpKind::FileRcx => BpKind::FileRcx,
                    _ => BpKind::FileR8,
                };
                if let Ok(Some(info)) = self.read_nt_file_info(thread_id, file_arg) {
                    let mut rewritten_to = None;
                    if self.rewrite_enabled {
                        if let Some(new_path) = self.should_rewrite(&info.path) {
                            if self.rewrite_path(&info, &new_path).is_ok() {
                                rewritten_to = Some(new_path.clone());
                                self.rewrite_count += 1;
                            }
                        }
                    }
                    self.file_accesses.push(FileAccess {
                        path: info.path,
                        rewritten_to,
                    });
                }
            }
            BpKind::NetConnect => {
                if let Ok(Some(net)) = self.read_connect_info(thread_id) {
                    self.net_accesses.push(net);
                }
            }
            BpKind::NetBind => {
                if let Err(e) = self.handle_bind(thread_id) {
                    if crate::is_verbose() { eprintln!("[cell-guard] bind interception error: {e}"); }
                }
            }
        }

        // Restore original byte so the real function can execute
        self.restore_byte(&bp)?;

        // Rewind RIP to the breakpoint address (INT3 advanced it by 1)
        let (handle, mut ctx) = get_context(thread_id, CONTEXT_CONTROL_FLAG)?;
        ctx.rip = bp.address;
        ctx.eflags |= 0x100; // Set Trap Flag for single-step
        set_context(handle, &ctx)?;
        let _ = unsafe { CloseHandle(handle) };

        self.pending_single_step = Some((thread_id, bp.address));
        Ok(true)
    }

    /// Handle EXCEPTION_SINGLE_STEP. Returns true if it was from our interceptor.
    pub fn handle_single_step(&mut self, thread_id: u32) -> Result<bool> {
        if let Some((step_tid, bp_addr)) = self.pending_single_step.take() {
            if step_tid == thread_id {
                // Re-arm the breakpoint now that we've stepped past it
                self.rearm_breakpoint(bp_addr);

                // Clear trap flag
                let (handle, mut ctx) = get_context(thread_id, CONTEXT_CONTROL_FLAG)?;
                ctx.eflags &= !0x100;
                set_context(handle, &ctx)?;
                let _ = unsafe { CloseHandle(handle) };

                return Ok(true);
            }
            // Wasn't our thread — put it back
            self.pending_single_step = Some((step_tid, bp_addr));
        }
        Ok(false)
    }

    /// Read the file path from an NT syscall's OBJECT_ATTRIBUTES argument.
    fn read_nt_file_info(&self, thread_id: u32, kind: BpKind) -> Result<Option<NtFileInfo>> {
        let (handle, ctx) = get_context(thread_id, CONTEXT_FULL_FLAG)?;
        let _ = unsafe { CloseHandle(handle) };

        let obj_attr_ptr = match kind {
            BpKind::FileRcx => ctx.rcx,
            BpKind::FileR8 => ctx.r8,
            _ => return Ok(None),
        };
        if obj_attr_ptr == 0 {
            return Ok(None);
        }

        // Read OBJECT_ATTRIBUTES from target
        let mut obj_attr: ObjectAttributes = unsafe { mem::zeroed() };
        let mut n: usize = 0;
        let read_ok = unsafe {
            ReadProcessMemory(
                self.process,
                obj_attr_ptr as *const _,
                &mut obj_attr as *mut _ as *mut _,
                mem::size_of::<ObjectAttributes>(),
                Some(&mut n),
            )
        };
        if read_ok.is_err() || obj_attr.object_name == 0 {
            return Ok(None);
        }

        // Read UNICODE_STRING
        let mut ustr: UnicodeString = unsafe { mem::zeroed() };
        let read_ok = unsafe {
            ReadProcessMemory(
                self.process,
                obj_attr.object_name as *const _,
                &mut ustr as *mut _ as *mut _,
                mem::size_of::<UnicodeString>(),
                Some(&mut n),
            )
        };
        if read_ok.is_err() || ustr.buffer == 0 || ustr.length == 0 {
            return Ok(None);
        }

        // Read the wide string
        let wide_len = (ustr.length / 2) as usize;
        if wide_len > 4096 {
            return Ok(None);
        }
        let mut wide_buf: Vec<u16> = vec![0u16; wide_len];

        let read_ok = unsafe {
            ReadProcessMemory(
                self.process,
                ustr.buffer as *const _,
                wide_buf.as_mut_ptr() as *mut _,
                ustr.length as usize,
                Some(&mut n),
            )
        };
        if read_ok.is_err() {
            return Ok(None);
        }

        Ok(Some(NtFileInfo {
            path: String::from_utf16_lossy(&wide_buf),
            ustr_addr: obj_attr.object_name,
            ustr,
            max_len: ustr.maximum_length,
        }))
    }

    /// Determine if a path should be rewritten into a volume mount or the container's rootfs.
    /// Returns the new NT path if rewriting should happen, None otherwise.
    ///
    /// Uses a whitelist approach: only rewrite paths whose first directory component
    /// actually exists in the rootfs (e.g. `app`, `bin`, `etc`, `data`). This avoids
    /// false positives on host paths like `Users`, `Python314`, `.rustup`, etc.
    fn should_rewrite(&self, nt_path: &str) -> Option<String> {
        if !nt_path.starts_with("\\??\\") { return None; }
        if nt_path.starts_with(&self.rootfs_nt) { return None; }

        let lower = nt_path.to_lowercase();
        if lower.contains("\\.cell\\") { return None; }

        // Skip system paths
        if lower.contains("\\windows\\") || lower.contains("\\program files")
           || lower.contains("\\programdata\\") { return None; }
        // Skip executables and system files
        if lower.ends_with(".dll") || lower.ends_with(".exe") || lower.ends_with(".sys")
           || lower.ends_with(".drv") || lower.ends_with(".nls") || lower.ends_with(".mui") { return None; }

        let win_path = &nt_path[4..];
        if win_path.len() <= 3 { return None; }

        // Check volume mounts first (priority)
        for (container_path_nt, host_volume_nt) in &self.volume_mounts {
            let nt_lower = nt_path.to_lowercase();
            let container_lower = container_path_nt.to_lowercase();
            if nt_lower.starts_with(&container_lower) {
                let rest = &nt_path[container_path_nt.len()..];
                return Some(format!("{}{}", host_volume_nt, rest));
            }
        }

        // Only rewrite if the first path component exists in rootfs
        let relative = if win_path.len() > 2 && win_path.as_bytes()[1] == b':' {
            &win_path[2..]
        } else {
            win_path
        };
        let relative_clean = relative.trim_start_matches('\\');

        // Get first directory component
        let first_component = relative_clean.split('\\').next().unwrap_or("");
        if first_component.is_empty() { return None; }

        // Check if this top-level dir exists in our rootfs
        let rootfs_path = std::path::Path::new(&self.rootfs_display);
        let check_path = rootfs_path.join(first_component);
        if !check_path.exists() { return None; }

        let new_path = format!("{}{}", self.rootfs_nt, relative);
        Some(new_path)
    }

    /// Rewrite the file path in the target process memory.
    /// If the new path is longer than the existing buffer, allocates new memory
    /// in the target process via VirtualAllocEx.
    fn rewrite_path(&self, info: &NtFileInfo, new_path: &str) -> Result<()> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        let wide_new: Vec<u16> = OsStr::new(new_path).encode_wide().collect();
        let new_byte_len = (wide_new.len() * 2) as u16;
        let mut n: usize = 0;

        let buffer_addr = if new_byte_len <= info.max_len {
            // New path fits in the existing buffer — write in-place.
            info.ustr.buffer
        } else {
            // New path is longer — allocate new memory in the target process.
            let alloc_size = new_byte_len as usize + 2; // +2 for null terminator
            let ptr = unsafe {
                VirtualAllocEx(
                    self.process,
                    None,
                    alloc_size,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            if ptr.is_null() {
                anyhow::bail!("VirtualAllocEx failed for path rewrite");
            }
            ptr as u64
        };

        // Write the new wide string
        unsafe {
            WriteProcessMemory(
                self.process,
                buffer_addr as *const _,
                wide_new.as_ptr() as *const _,
                new_byte_len as usize,
                Some(&mut n),
            )
            .context("WriteProcessMemory (path rewrite) failed")?;
        }

        // Update the UNICODE_STRING to point to the new buffer with new length
        let mut new_ustr = info.ustr;
        new_ustr.length = new_byte_len;
        new_ustr.maximum_length = new_byte_len + 2;
        new_ustr.buffer = buffer_addr;

        unsafe {
            WriteProcessMemory(
                self.process,
                info.ustr_addr as *const _,
                &new_ustr as *const _ as *const _,
                mem::size_of::<UnicodeString>(),
                Some(&mut n),
            )
            .context("WriteProcessMemory (ustr update) failed")?;
        }

        Ok(())
    }

    /// Read the sockaddr from a Winsock connect() call.
    /// connect(SOCKET, sockaddr*, namelen) — RCX=socket, RDX=sockaddr*, R8=namelen
    fn read_connect_info(&self, thread_id: u32) -> Result<Option<NetAccess>> {
        let (handle, ctx) = get_context(thread_id, CONTEXT_FULL_FLAG)?;
        let _ = unsafe { CloseHandle(handle) };

        let sockaddr_ptr = ctx.rdx;
        let namelen = ctx.r8 as usize;

        if sockaddr_ptr == 0 || namelen < 4 {
            return Ok(None);
        }

        // Read the first 2 bytes to get address family
        let mut sa_family: u16 = 0;
        let mut n: usize = 0;
        let read_ok = unsafe {
            ReadProcessMemory(
                self.process,
                sockaddr_ptr as *const _,
                &mut sa_family as *mut _ as *mut _,
                2,
                Some(&mut n),
            )
        };
        if read_ok.is_err() {
            return Ok(None);
        }

        // AF_INET = 2
        if sa_family == 2 && namelen >= 16 {
            // sockaddr_in: family(2) + port(2) + addr(4) + zero(8)
            let mut buf = [0u8; 16];
            let read_ok = unsafe {
                ReadProcessMemory(
                    self.process,
                    sockaddr_ptr as *const _,
                    buf.as_mut_ptr() as *mut _,
                    16,
                    Some(&mut n),
                )
            };
            if read_ok.is_err() {
                return Ok(None);
            }

            let port = u16::from_be_bytes([buf[2], buf[3]]);
            let addr = format!("{}.{}.{}.{}", buf[4], buf[5], buf[6], buf[7]);

            let blocked = !self.allowed_ports.is_empty() && !self.allowed_ports.contains(&port);

            if crate::is_verbose() {
                eprintln!(
                    "[cell-guard] network: connect to {}:{}{}",
                    addr, port,
                    if blocked { " [BLOCKED]" } else { "" }
                );
            }

            return Ok(Some(NetAccess { addr, port, blocked }));
        }

        // AF_INET6 = 23
        if sa_family == 23 && namelen >= 28 {
            let mut buf = [0u8; 28];
            let read_ok = unsafe {
                ReadProcessMemory(
                    self.process,
                    sockaddr_ptr as *const _,
                    buf.as_mut_ptr() as *mut _,
                    28,
                    Some(&mut n),
                )
            };
            if read_ok.is_err() {
                return Ok(None);
            }

            let port = u16::from_be_bytes([buf[2], buf[3]]);
            // Simple IPv6 display — first/last 4 bytes
            let addr = format!(
                "[{:02x}{:02x}::{:02x}{:02x}]",
                buf[8], buf[9], buf[22], buf[23]
            );

            let blocked = !self.allowed_ports.is_empty() && !self.allowed_ports.contains(&port);

            if crate::is_verbose() {
                eprintln!(
                    "[cell-guard] network: connect to {}:{}{}",
                    addr, port,
                    if blocked { " [BLOCKED]" } else { "" }
                );
            }

            return Ok(Some(NetAccess { addr, port, blocked }));
        }

        Ok(None)
    }

    /// Handle a `bind()` call: if the container_port matches a port mapping, rewrite the
    /// sockaddr in target process memory to use the host_port instead.
    /// bind(SOCKET, sockaddr*, namelen) — RCX=socket, RDX=sockaddr*, R8=namelen
    fn handle_bind(&self, thread_id: u32) -> Result<()> {
        if self.port_mappings.is_empty() {
            return Ok(());
        }

        let (handle, ctx) = get_context(thread_id, CONTEXT_FULL_FLAG)?;
        let _ = unsafe { CloseHandle(handle) };

        let sockaddr_ptr = ctx.rdx;
        let namelen = ctx.r8 as usize;

        if sockaddr_ptr == 0 || namelen < 4 {
            return Ok(());
        }

        // Read address family
        let mut sa_family: u16 = 0;
        let mut n: usize = 0;
        let read_ok = unsafe {
            ReadProcessMemory(
                self.process,
                sockaddr_ptr as *const _,
                &mut sa_family as *mut _ as *mut _,
                2,
                Some(&mut n),
            )
        };
        if read_ok.is_err() {
            return Ok(());
        }

        // AF_INET = 2
        if sa_family == 2 && namelen >= 16 {
            let mut buf = [0u8; 16];
            let read_ok = unsafe {
                ReadProcessMemory(
                    self.process,
                    sockaddr_ptr as *const _,
                    buf.as_mut_ptr() as *mut _,
                    16,
                    Some(&mut n),
                )
            };
            if read_ok.is_err() {
                return Ok(());
            }

            let container_port = u16::from_be_bytes([buf[2], buf[3]]);

            // Find a matching port mapping
            if let Some(&(host_port, _)) = self
                .port_mappings
                .iter()
                .find(|&&(_, cp)| cp == container_port)
            {
                if crate::is_verbose() {
                    eprintln!(
                        "[cell-guard] port rewrite: bind container:{} -> host:{}",
                        container_port, host_port
                    );
                }
                // Overwrite bytes 2-3 of the sockaddr with the host port in big-endian
                let host_port_be = host_port.to_be_bytes();
                let mut n2: usize = 0;
                let _ = unsafe {
                    WriteProcessMemory(
                        self.process,
                        (sockaddr_ptr + 2) as *const _,
                        host_port_be.as_ptr() as *const _,
                        2,
                        Some(&mut n2),
                    )
                };
            }
        }

        Ok(())
    }

    /// Remove all breakpoints (restore original bytes).
    pub fn cleanup(&self) {
        for bp in self.breakpoints.values() {
            let _ = self.restore_byte(bp);
        }
    }

    /// Print a summary of intercepted accesses.
    pub fn print_summary(&self) {
        // Network summary
        if !self.net_accesses.is_empty() {
            eprintln!("[cell-guard] network connections ({}):", self.net_accesses.len());
            for net in &self.net_accesses {
                eprintln!(
                    "  {}:{}{}",
                    net.addr,
                    net.port,
                    if net.blocked { " [BLOCKED]" } else { "" }
                );
            }
        }

        if self.file_accesses.is_empty() {
            return;
        }

        // Show rewrites first
        let rewrites: Vec<&FileAccess> = self
            .file_accesses
            .iter()
            .filter(|a| a.rewritten_to.is_some())
            .collect();

        if !rewrites.is_empty() {
            println!("[cell-guard] path rewrites ({}):", rewrites.len());
            let mut seen = std::collections::HashSet::new();
            for access in &rewrites {
                if seen.insert(&access.path) {
                    println!(
                        "  {} -> {}",
                        access.path,
                        access.rewritten_to.as_deref().unwrap_or("?")
                    );
                }
            }
        }

        // Deduplicated access summary
        let mut unique_paths: Vec<&str> = self
            .file_accesses
            .iter()
            .map(|a| a.path.as_str())
            .collect();
        unique_paths.sort();
        unique_paths.dedup();

        println!(
            "[cell-guard] file access summary ({} unique paths, {} calls, {} rewrites):",
            unique_paths.len(),
            self.file_accesses.len(),
            self.rewrite_count,
        );

        for (i, path) in unique_paths.iter().take(20).enumerate() {
            println!("  {}. {}", i + 1, path);
        }
        if unique_paths.len() > 20 {
            println!("  ... and {} more", unique_paths.len() - 20);
        }
    }
}
