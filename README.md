# Cell

**Containers without kernel namespaces.** Cell builds isolation from first principles via syscall interception — no daemon, rootless, truly cross-platform.

Docker asked Linux for containers. Cell **builds its own** from userspace.

## How it works

```
+------------------+
|  Your Process    |
+--------+---------+
         | syscall
+--------v---------+
|    Cell Guard     |  intercepts, rewrites, or denies
|  (per-OS debug    |
|   API)            |
+--------+---------+
         | real syscall (maybe rewritten)
+--------v---------+
|    OS Kernel      |
+------------------+
```

Cell Guard sits between the contained process and the kernel. It intercepts every relevant syscall using the OS's native debugging API:

- **Windows**: `CreateProcess(DEBUG_PROCESS)` + `WaitForDebugEvent` + INT3 breakpoints
- **Linux**: `ptrace(PTRACE_SYSCALL)` + seccomp-BPF *(coming soon)*
- **macOS**: `ptrace` + Sandbox (Seatbelt) *(coming soon)*

### What gets intercepted

| Syscall | What Cell does |
|---------|---------------|
| `NtCreateFile` / `NtOpenFile` | Rewrites file paths to container rootfs |
| `NtQueryAttributesFile` | Same — catches `stat()`, `if exist` checks |
| `NtDeleteFile` | Redirects file deletion to rootfs |
| `bind()` | Rewrites port number for zero-cost port forwarding |
| `connect()` | Logs and optionally blocks outbound connections |

### Zero-cost port forwarding

Docker spawns a proxy process per port. Cell rewrites the port number in the `sockaddr` struct via `WriteProcessMemory`. No proxy. No iptables. One memory write.

### Volumes via path rewrite

Docker needs volume drivers and overlay filesystems. Cell redirects file paths to `~/.cell/volumes/<name>/` using the same syscall interception. Direct host directory access at native speed.

## Quick start

```bash
# Build from a Cellfile
cell build

# Pull a Docker image and convert it
cell pull alpine:3.19

# Run a container
cell run myapp

# Run with verbose guard output
cell -v run myapp

# Interactive mode
cell run -i myapp "cmd.exe"

# Execute in an existing container
cell exec <container-id> "cmd.exe /c dir"

# List images and containers
cell images
cell ps

# Stop and remove
cell stop <container-id>
cell rm <container-id>

# Convert a Dockerfile
cell convert Dockerfile

# Platform info
cell info
```

## Cellfile reference

```
cell {
  name = "myapp"
  base = "alpine:3.19"           # auto-pull base image

  env {
    NODE_ENV = "production"
    PORT = "3000"
  }

  fs {
    copy "src/" to "/app/src"
    copy "config.json" to "/app/"
  }

  run = "node /app/src/server.js"
  expose = [3000, 8080]

  ports {
    9090 = 80                    # host:9090 -> container:80
    3001 = 3000
  }

  volumes {
    "appdata" = "/app/data"      # ~/.cell/volumes/appdata/ <-> /app/data
    "logs" = "/var/log"
  }

  limits {
    memory = "512MB"
    processes = 10
  }
}
```

## Architecture

```
cell/
|-- cell-cli/        CLI binary (build, run, exec, pull, images, ps, rm, stop, info, convert)
|-- cell-format/     Cellfile lexer + recursive-descent parser + image manifest types
|-- cell-store/      Content-addressed blob store (SHA-256), image + container state
|-- cell-runtime/    Cell Guard: Debug API, Job Objects, syscall interception + rewriting
|-- cell-oci/        Docker Hub pull, OCI tar extraction, Dockerfile conversion
```

### Key design decisions

- **Hand-written parser** for Cellfiles (not YAML/TOML) — supports `copy "x" to "y"` syntax, great error messages
- **Content-addressed storage** with SHA-256 dedup — same blob is never stored twice
- **Raw FFI for thread context** — `GetThreadContext`/`SetThreadContext` via `extern "system"` because the `windows` crate doesn't expose them
- **VirtualAllocEx for path rewriting** — when the rewritten path is longer than the original buffer, we allocate new memory in the target process
- **Whitelist-based path rewriting** — only rewrites paths whose first directory component exists in the rootfs (no false positives)
- **Job Objects for resource limits** — `KILL_ON_JOB_CLOSE` means Ctrl+C automatically cleans up all container processes

## Docker compatibility

```bash
# Pull any public Docker image
cell pull nginx:latest

# Convert a Dockerfile to a Cellfile
cell convert Dockerfile

# Base images are auto-pulled during build
cell build   # if Cellfile has base = "node:18"
```

Cell speaks OCI. It pulls from Docker Hub, extracts layers, and converts to its native format. Bidirectional — your images are stored as content-addressed blobs, not opaque tarballs.

## Platform isolation levels

| | Linux | Windows | macOS |
|---|---|---|---|
| Method | ptrace + seccomp-BPF | Debug API + Job Objects | ptrace + Seatbelt |
| Filesystem | Intercepted | Intercepted | Intercepted |
| Network | Intercepted | Intercepted | Intercepted |
| Resources | cgroups | Job Objects (full) | setrlimit |
| Status | Coming soon | **Working** | Coming soon |

## Building from source

```bash
# Requires Rust 1.70+
cargo build --release

# Run tests
cargo test --all

# The binary is at target/release/cell (or cell.exe on Windows)
```

## License

MIT
