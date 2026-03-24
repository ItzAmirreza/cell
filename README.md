# Cell

A container runtime built from first principles. No daemon. No VM. No kernel namespaces required.

Cell isolates processes by intercepting their syscalls — sitting between the contained process and the kernel, rewriting file paths, filtering network connections, and enforcing resource limits. Same CLI, same Cellfile, same mental model on every OS.

## How it works

```
+------------------+
|  Contained       |
|   Process        |
+--------+---------+
         | syscall
+--------v---------+
|  Cell Guard       |  <-- intercepts, rewrites, denies, or passes through
|  (ptrace on Linux |
|   Debug API on    |
|   Windows)        |
+--------+---------+
         | real syscall (maybe rewritten)
+--------v---------+
|   OS Kernel       |
+------------------+
```

On Linux, Cell Guard uses `ptrace(PTRACE_SYSCALL)` to intercept every syscall from the child process. When it sees a file open (`openat`, `stat`, `access`, etc.), it reads the path from the process's memory, and if the file exists in the container's rootfs, rewrites the path to point there. The process reads containerized files without knowing they were redirected.

Network syscalls (`connect`, `bind`, `listen`) are intercepted and filtered by port policy. Resource limits are enforced via cgroups v2.

## Quick start

```sh
cargo build

# Build an image from a Cellfile
cell build Cellfile.example

# Pull an image from Docker Hub
cell pull alpine:3.19

# Run a container
cell run alpine_3.19 "/bin/sh -c 'echo hello from alpine && cat /etc/alpine-release'"

# List images and containers
cell images
cell ps

# Show platform isolation info
cell info
```

## The Cellfile format

Cellfiles are a declarative, typed replacement for Dockerfiles. No imperative shell scripts, no layer caching surprises.

```
cell {
  name = "myapp"
  base = "alpine:3.19"    # auto-pulled from Docker Hub

  env {
    NODE_ENV = "production"
    PORT = "8080"
  }

  fs {
    copy "src/" to "/app/src"
    copy "config.json" to "/app/config.json"
  }

  run = "/app/start.sh"
  expose = [8080, 443]

  limits {
    memory = "512MB"
    processes = 10
  }
}
```

## Commands

| Command | Description |
|---|---|
| `cell build <Cellfile>` | Build an image from a Cellfile |
| `cell run <image> [command]` | Run a container from an image |
| `cell pull <reference>` | Pull an OCI image from a registry (Docker Hub, ghcr.io, etc.) |
| `cell images` | List locally stored images |
| `cell ps` | List containers |
| `cell rm <id>` | Remove a container |
| `cell convert <Dockerfile>` | Convert a Dockerfile to a Cellfile |
| `cell info` | Show platform isolation capabilities |

## Docker compatibility

Cell can pull any public Docker/OCI image and convert it to its native format:

```sh
cell pull nginx:latest
cell pull ghcr.io/owner/repo:v1
cell convert Dockerfile          # generates a Cellfile
```

When building with `base = "alpine:3.19"`, Cell auto-pulls the base image and layers it beneath the container's own files.

## Architecture

```
cell/
+-- cell-format/     Cellfile lexer + recursive-descent parser
+-- cell-store/      Content-addressed blob store (SHA-256), image manifests, container state
+-- cell-runtime/    Cell Guard -- syscall interception per OS
|   +-- linux/       ptrace(PTRACE_SYSCALL) + seccomp + cgroups v2
|   +-- windows/     Debug API + Job Objects (real on Windows builds)
|   +-- macos/       ptrace + Seatbelt (stub)
+-- cell-oci/        OCI registry client, image pull/convert/export
+-- cell-cli/        CLI binary (clap)
```

### What Cell Guard intercepts (Linux)

**Filesystem** (30+ syscall types): `openat`, `stat`, `access`, `execve`, `mkdir`, `unlink`, `rename`, `chmod`, `chown`, `readlink`, `statx`, and their `*at` variants. Paths are rewritten into the container's rootfs via `PTRACE_PEEKDATA`/`PTRACE_POKEDATA`.

**Network**: `connect`, `bind`, `listen`. Sockaddr is parsed from tracee memory to extract IP/port. Connections to disallowed ports are blocked by setting `orig_rax = -1` (returns `ENOSYS`).

**Resources**: cgroups v2 for memory and process limits (requires root or cgroup delegation).

### Content-addressed store

```
~/.cell/
+-- store/
|   +-- blobs/sha256-<hex>              raw blobs, deduped by hash
|   +-- images/<name>/manifest.json     image manifests
+-- containers/
    +-- <id>/state.json                 container lifecycle state
    +-- <id>/rootfs/                    extracted filesystem
```

## How it differs from Docker

| | Docker | Cell |
|---|---|---|
| Daemon | dockerd (long-running, root) | None. Each container is a child process. |
| Isolation | Linux namespaces only | Syscall interception. Works on any OS with ptrace/debug APIs. |
| Build format | Dockerfile (imperative shell) | Cellfile (declarative, typed) |
| Image format | Stacked tarballs | Content-addressed blobs (SHA-256) |
| Cross-platform | VM on macOS/Windows | Native isolation per OS |
| Registry | Docker Hub monoculture | Pulls from any OCI registry, federated by design |

## Building from source

```sh
# Requires Rust 1.70+
cargo build --release

# Run tests
cargo test --all

# The binary is at target/release/cell
```

## License

MIT
