use std::net::Ipv4Addr;
use std::path::PathBuf;

/// A NAT rule that rewrites network destination addresses at the syscall level.
#[derive(Debug, Clone)]
pub struct NatRule {
    /// Hostname or IP to match (e.g. "db" or "10.0.0.1").
    pub match_host: String,
    /// Port to match (e.g. 5432).
    pub match_port: u16,
    /// Replacement host/IP (e.g. "10.0.0.5").
    pub target_host: String,
    /// Replacement port (e.g. 5432).
    pub target_port: u16,
}

impl NatRule {
    /// Check if a given (host, port) pair matches this NAT rule.
    pub fn matches(&self, host: &str, port: u16) -> bool {
        self.match_port == port && self.match_host == host
    }

    /// Parse the target_host as an IPv4 address.
    /// Returns None if the target_host is not a valid IPv4 address.
    pub fn target_ipv4(&self) -> Option<Ipv4Addr> {
        self.target_host.parse::<Ipv4Addr>().ok()
    }
}

/// Rules for rewriting syscalls in the contained process.
#[derive(Debug, Clone)]
pub struct RewriteRules {
    /// The container's root filesystem path on the host.
    pub rootfs: PathBuf,
    /// Fake PID to report to the contained process.
    pub fake_pid: u32,
    /// The real PID of the main child process (for /proc/<pid> rewriting).
    pub real_pid: u32,
    /// Allowed outbound connection ports (empty = allow all).
    pub allowed_ports: Vec<u16>,
    /// Allowed bind ports (empty = allow all).
    pub allowed_bind_ports: Vec<u16>,
    /// Network address translation rules for connect() syscalls.
    pub nat_rules: Vec<NatRule>,
}

/// /proc paths that should be passed through to the host (not virtualized).
const PROC_PASSTHROUGH: &[&str] = &[
    "/proc/sys",
    "/proc/meminfo",
    "/proc/cpuinfo",
    "/proc/filesystems",
    "/proc/mounts",
    "/proc/loadavg",
    "/proc/uptime",
    "/proc/version",
    "/proc/stat",
    "/proc/net",
    "/proc/bus",
    "/proc/irq",
    "/proc/devices",
    "/proc/misc",
    "/proc/modules",
    "/proc/partitions",
    "/proc/swaps",
    "/proc/vmstat",
    "/proc/zoneinfo",
    "/proc/kallsyms",
    "/proc/interrupts",
];

/// /proc/self (and /proc/<pid>) sub-paths that we virtualize.
const PROC_VIRTUALIZED: &[&str] = &[
    "status",
    "stat",
    "cmdline",
];

impl RewriteRules {
    /// Check if a /proc path should be rewritten to our virtual /proc files.
    /// Returns the rewritten path if so, None otherwise (pass-through).
    fn rewrite_proc_path(&self, path: &str) -> Option<String> {
        let rootfs_str = self.rootfs.to_string_lossy();

        // Check if it's a /proc/self/<file> or /proc/<real_pid>/<file> path
        let suffix = if let Some(rest) = path.strip_prefix("/proc/self/") {
            Some(rest)
        } else {
            let pid_prefix = format!("/proc/{}/", self.real_pid);
            path.strip_prefix(&pid_prefix)
        };

        if let Some(suffix) = suffix {
            // Only virtualize known sub-paths
            let base_suffix = suffix.split('/').next().unwrap_or(suffix);
            if PROC_VIRTUALIZED.contains(&base_suffix) {
                let new_path = format!("{}/proc/self/{}", rootfs_str, suffix);
                let candidate = std::path::Path::new(&new_path);
                if !self.rootfs.exists() || candidate.exists() {
                    return Some(new_path);
                }
            }
        }

        // Also handle bare /proc/self and /proc/<pid> directory access
        if path == "/proc/self" || path == format!("/proc/{}", self.real_pid) {
            let new_path = format!("{}/proc/self", rootfs_str);
            let candidate = std::path::Path::new(&new_path);
            if !self.rootfs.exists() || candidate.exists() || candidate.is_dir() {
                return Some(new_path);
            }
        }

        None
    }

    /// Determine if a file path should be rewritten into the container's rootfs.
    /// Returns the new path if rewriting should happen, None otherwise.
    pub fn rewrite_path(&self, path: &str) -> Option<String> {
        // Skip empty paths
        if path.is_empty() {
            return None;
        }

        // Only rewrite absolute paths
        if !path.starts_with('/') {
            return None;
        }

        // Skip if already inside the rootfs
        let rootfs_str = self.rootfs.to_string_lossy();
        if path.starts_with(rootfs_str.as_ref()) {
            return None;
        }

        // Handle /proc paths specially: virtualize some, pass through others
        if path.starts_with("/proc") {
            // Check if this is a pass-through /proc path
            for passthrough in PROC_PASSTHROUGH {
                if path.starts_with(passthrough) {
                    return None;
                }
            }
            // Try to rewrite /proc/self and /proc/<pid> paths to virtual files
            return self.rewrite_proc_path(path);
        }

        // Skip system paths the process needs to function
        if path.starts_with("/sys")
            || path.starts_with("/dev")
            || path.starts_with("/bin")
            || path.starts_with("/sbin")
            || path.starts_with("/usr")
            || path.starts_with("/lib")
            || path.starts_with("/lib64")
            || path.starts_with("/run")
            || path.starts_with("/tmp")
            || path.starts_with("/var")
            || path.starts_with("/nix")
        {
            return None;
        }

        // Skip paths inside .cell directory (our own infrastructure)
        if path.contains("/.cell/") {
            return None;
        }

        // Rewrite: prepend rootfs path.
        // Only rewrite if the target file or its parent directory exists in the rootfs.
        // This prevents breaking accesses to files the container doesn't provide
        // (e.g. the dynamic linker looking for libs in the user's home dir).
        let new_path = format!("{}{}", rootfs_str, path);
        let candidate = std::path::Path::new(&new_path);

        // If the rootfs itself doesn't exist on disk (e.g. in tests), always rewrite
        if !self.rootfs.exists() {
            return Some(new_path);
        }

        if candidate.exists()
            || candidate.parent().map_or(false, |p| p.is_dir())
        {
            Some(new_path)
        } else {
            None
        }
    }

    /// Check if an outbound connection port is allowed.
    pub fn port_allowed(&self, port: u16) -> bool {
        self.allowed_ports.is_empty() || self.allowed_ports.contains(&port)
    }

    /// Check if a bind port is allowed.
    pub fn bind_port_allowed(&self, port: u16) -> bool {
        self.allowed_bind_ports.is_empty() || self.allowed_bind_ports.contains(&port)
    }

    /// Determine if a file should be copied on write. Returns true when:
    /// - The flags indicate write intent (O_WRONLY, O_RDWR, or O_CREAT)
    /// - The file exists on the host at `host_path`
    /// - The file does NOT already exist in the rootfs
    ///
    /// Only handles regular files.
    pub fn should_copy_on_write(&self, host_path: &str, flags: u64) -> bool {
        const O_WRONLY: u64 = 1;
        const O_RDWR: u64 = 2;
        const O_CREAT: u64 = 64;

        let write_intent = (flags & O_WRONLY) != 0
            || (flags & O_RDWR) != 0
            || (flags & O_CREAT) != 0;

        if !write_intent {
            return false;
        }

        let host = std::path::Path::new(host_path);
        if !host.is_file() {
            return false;
        }

        let rootfs_str = self.rootfs.to_string_lossy();
        let rootfs_path = format!("{}{}", rootfs_str, host_path);
        let dest = std::path::Path::new(&rootfs_path);

        !dest.exists()
    }

    /// Build the rootfs-rewritten path for a given host absolute path.
    /// This is the destination path used for copy-on-write.
    pub fn rootfs_target(&self, host_path: &str) -> String {
        let rootfs_str = self.rootfs.to_string_lossy();
        format!("{}{}", rootfs_str, host_path)
    }

    /// Check if flags indicate write intent (O_WRONLY | O_RDWR | O_CREAT).
    pub fn has_write_intent(flags: u64) -> bool {
        const O_WRONLY: u64 = 1;
        const O_RDWR: u64 = 2;
        const O_CREAT: u64 = 64;

        (flags & O_WRONLY) != 0 || (flags & O_RDWR) != 0 || (flags & O_CREAT) != 0
    }

    /// Look up a matching NAT rule for a given (host, port) pair.
    /// Returns the first matching rule, or None.
    pub fn lookup_nat(&self, host: &str, port: u16) -> Option<&NatRule> {
        self.nat_rules.iter().find(|rule| rule.matches(host, port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rules() -> RewriteRules {
        RewriteRules {
            rootfs: PathBuf::from("/home/user/.cell/containers/abc123/rootfs"),
            fake_pid: 1,
            real_pid: 0,
            allowed_ports: vec![8080, 3000],
            allowed_bind_ports: vec![8080, 3000],
            nat_rules: vec![],
        }
    }

    #[test]
    fn rewrite_absolute_path() {
        let rules = test_rules();
        let result = rules.rewrite_path("/etc/hosts");
        assert_eq!(
            result,
            Some("/home/user/.cell/containers/abc123/rootfs/etc/hosts".into())
        );
    }

    #[test]
    fn skip_proc_sys_dev() {
        let rules = test_rules();
        // /proc/self/status is now virtualized (rewritten to rootfs)
        assert_eq!(
            rules.rewrite_path("/proc/self/status"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self/status".into())
        );
        // /proc/sys is pass-through
        assert_eq!(rules.rewrite_path("/proc/sys/net/core"), None);
        assert_eq!(rules.rewrite_path("/sys/class/net"), None);
        assert_eq!(rules.rewrite_path("/dev/null"), None);
    }

    #[test]
    fn proc_virtualization() {
        let rules = RewriteRules {
            rootfs: PathBuf::from("/home/user/.cell/containers/abc123/rootfs"),
            fake_pid: 1,
            real_pid: 4567,
            allowed_ports: vec![],
            allowed_bind_ports: vec![],
            nat_rules: vec![],
        };

        // /proc/self/status, stat, cmdline are virtualized
        assert_eq!(
            rules.rewrite_path("/proc/self/status"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self/status".into())
        );
        assert_eq!(
            rules.rewrite_path("/proc/self/stat"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self/stat".into())
        );
        assert_eq!(
            rules.rewrite_path("/proc/self/cmdline"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self/cmdline".into())
        );

        // /proc/<real_pid>/status is also virtualized
        assert_eq!(
            rules.rewrite_path("/proc/4567/status"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self/status".into())
        );
        assert_eq!(
            rules.rewrite_path("/proc/4567/cmdline"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self/cmdline".into())
        );

        // /proc passthrough paths are NOT rewritten
        assert_eq!(rules.rewrite_path("/proc/meminfo"), None);
        assert_eq!(rules.rewrite_path("/proc/cpuinfo"), None);
        assert_eq!(rules.rewrite_path("/proc/sys/net/core"), None);

        // /proc/self/maps (not in PROC_VIRTUALIZED) is not rewritten
        assert_eq!(rules.rewrite_path("/proc/self/maps"), None);

        // Bare /proc/self is rewritten (non-existent rootfs -> always rewrite)
        assert_eq!(
            rules.rewrite_path("/proc/self"),
            Some("/home/user/.cell/containers/abc123/rootfs/proc/self".into())
        );
    }

    #[test]
    fn skip_system_paths() {
        let rules = test_rules();
        assert_eq!(rules.rewrite_path("/lib/x86_64-linux-gnu/libc.so.6"), None);
        assert_eq!(rules.rewrite_path("/usr/lib/libssl.so"), None);
        assert_eq!(rules.rewrite_path("/bin/sh"), None);
        assert_eq!(rules.rewrite_path("/usr/bin/env"), None);
    }

    #[test]
    fn skip_already_in_rootfs() {
        let rules = test_rules();
        assert_eq!(
            rules.rewrite_path("/home/user/.cell/containers/abc123/rootfs/etc/hosts"),
            None
        );
    }

    #[test]
    fn skip_relative() {
        let rules = test_rules();
        assert_eq!(rules.rewrite_path("relative/path"), None);
    }

    #[test]
    fn port_filtering() {
        let rules = test_rules();
        assert!(rules.port_allowed(8080));
        assert!(rules.port_allowed(3000));
        assert!(!rules.port_allowed(22));
    }

    #[test]
    fn empty_ports_allows_all() {
        let rules = RewriteRules {
            rootfs: PathBuf::from("/tmp"),
            fake_pid: 1,
            real_pid: 0,
            allowed_ports: vec![],
            allowed_bind_ports: vec![],
            nat_rules: vec![],
        };
        assert!(rules.port_allowed(22));
        assert!(rules.port_allowed(80));
    }

    #[test]
    fn bind_port_filtering() {
        let rules = test_rules();
        assert!(rules.bind_port_allowed(8080));
        assert!(rules.bind_port_allowed(3000));
        assert!(!rules.bind_port_allowed(22));
    }

    #[test]
    fn empty_bind_ports_allows_all() {
        let rules = RewriteRules {
            rootfs: PathBuf::from("/tmp"),
            fake_pid: 1,
            real_pid: 0,
            allowed_ports: vec![],
            allowed_bind_ports: vec![],
            nat_rules: vec![],
        };
        assert!(rules.bind_port_allowed(22));
        assert!(rules.bind_port_allowed(80));
    }

    // --- NatRule tests ---

    #[test]
    fn nat_rule_matches_exact() {
        let rule = NatRule {
            match_host: "10.0.0.1".into(),
            match_port: 5432,
            target_host: "10.0.0.5".into(),
            target_port: 5432,
        };
        assert!(rule.matches("10.0.0.1", 5432));
    }

    #[test]
    fn nat_rule_no_match_wrong_port() {
        let rule = NatRule {
            match_host: "10.0.0.1".into(),
            match_port: 5432,
            target_host: "10.0.0.5".into(),
            target_port: 5432,
        };
        assert!(!rule.matches("10.0.0.1", 3306));
    }

    #[test]
    fn nat_rule_no_match_wrong_host() {
        let rule = NatRule {
            match_host: "10.0.0.1".into(),
            match_port: 5432,
            target_host: "10.0.0.5".into(),
            target_port: 5432,
        };
        assert!(!rule.matches("10.0.0.2", 5432));
    }

    #[test]
    fn nat_rule_matches_hostname() {
        let rule = NatRule {
            match_host: "db".into(),
            match_port: 5432,
            target_host: "10.0.0.5".into(),
            target_port: 5432,
        };
        assert!(rule.matches("db", 5432));
        assert!(!rule.matches("cache", 5432));
    }

    #[test]
    fn nat_rule_target_ipv4_valid() {
        let rule = NatRule {
            match_host: "db".into(),
            match_port: 5432,
            target_host: "10.0.0.5".into(),
            target_port: 5432,
        };
        assert_eq!(rule.target_ipv4(), Some(Ipv4Addr::new(10, 0, 0, 5)));
    }

    #[test]
    fn nat_rule_target_ipv4_invalid() {
        let rule = NatRule {
            match_host: "db".into(),
            match_port: 5432,
            target_host: "not-an-ip".into(),
            target_port: 5432,
        };
        assert_eq!(rule.target_ipv4(), None);
    }

    #[test]
    fn lookup_nat_finds_first_match() {
        let rules = RewriteRules {
            rootfs: PathBuf::from("/tmp"),
            fake_pid: 1,
            real_pid: 0,
            allowed_ports: vec![],
            allowed_bind_ports: vec![],
            nat_rules: vec![
                NatRule {
                    match_host: "10.0.0.1".into(),
                    match_port: 5432,
                    target_host: "10.0.0.5".into(),
                    target_port: 5433,
                },
                NatRule {
                    match_host: "10.0.0.2".into(),
                    match_port: 3306,
                    target_host: "10.0.0.6".into(),
                    target_port: 3307,
                },
            ],
        };

        let found = rules.lookup_nat("10.0.0.1", 5432).unwrap();
        assert_eq!(found.target_host, "10.0.0.5");
        assert_eq!(found.target_port, 5433);

        let found2 = rules.lookup_nat("10.0.0.2", 3306).unwrap();
        assert_eq!(found2.target_host, "10.0.0.6");
        assert_eq!(found2.target_port, 3307);

        assert!(rules.lookup_nat("10.0.0.99", 5432).is_none());
    }

    #[test]
    fn lookup_nat_empty_rules() {
        let rules = RewriteRules {
            rootfs: PathBuf::from("/tmp"),
            fake_pid: 1,
            real_pid: 0,
            allowed_ports: vec![],
            allowed_bind_ports: vec![],
            nat_rules: vec![],
        };
        assert!(rules.lookup_nat("10.0.0.1", 5432).is_none());
    }

    #[test]
    fn has_write_intent_flags() {
        // O_RDONLY = 0
        assert!(!RewriteRules::has_write_intent(0));
        // O_WRONLY = 1
        assert!(RewriteRules::has_write_intent(1));
        // O_RDWR = 2
        assert!(RewriteRules::has_write_intent(2));
        // O_CREAT = 64
        assert!(RewriteRules::has_write_intent(64));
        // O_RDONLY | O_CREAT = 64 (create for reading, still write intent)
        assert!(RewriteRules::has_write_intent(64));
        // O_WRONLY | O_CREAT | O_TRUNC = 1 | 64 | 512 = 577
        assert!(RewriteRules::has_write_intent(577));
        // O_RDONLY | O_CLOEXEC = 0 | 0x80000 = no write intent
        assert!(!RewriteRules::has_write_intent(0x80000));
    }
}
