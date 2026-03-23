use std::path::PathBuf;

/// Rules for rewriting syscalls in the contained process.
#[derive(Debug, Clone)]
pub struct RewriteRules {
    /// The container's root filesystem path on the host.
    pub rootfs: PathBuf,
    /// Fake PID to report to the contained process.
    pub fake_pid: u32,
    /// Allowed network ports (empty = deny all).
    pub allowed_ports: Vec<u16>,
    /// Environment variables to inject.
    pub env: Vec<(String, String)>,
}

impl RewriteRules {
    /// Rewrite a path from the container's perspective to the host path.
    /// e.g., `/etc/passwd` → `<rootfs>/etc/passwd`
    pub fn rewrite_path(&self, container_path: &str) -> PathBuf {
        // Strip leading separator to avoid absolute path overriding the join
        let relative = container_path.trim_start_matches('/').trim_start_matches('\\');
        self.rootfs.join(relative)
    }

    /// Check if a network port is allowed.
    pub fn is_port_allowed(&self, port: u16) -> bool {
        self.allowed_ports.is_empty() || self.allowed_ports.contains(&port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_rules() -> RewriteRules {
        RewriteRules {
            rootfs: PathBuf::from("/tmp/cell/abc123/rootfs"),
            fake_pid: 1,
            allowed_ports: vec![8080, 3000],
            env: vec![],
        }
    }

    #[test]
    fn test_path_rewrite() {
        let rules = test_rules();
        let rewritten = rules.rewrite_path("/etc/passwd");
        assert_eq!(
            rewritten,
            Path::new("/tmp/cell/abc123/rootfs/etc/passwd")
        );
    }

    #[test]
    fn test_path_rewrite_nested() {
        let rules = test_rules();
        let rewritten = rules.rewrite_path("/app/src/main.rs");
        assert_eq!(
            rewritten,
            Path::new("/tmp/cell/abc123/rootfs/app/src/main.rs")
        );
    }

    #[test]
    fn test_port_allowed() {
        let rules = test_rules();
        assert!(rules.is_port_allowed(8080));
        assert!(!rules.is_port_allowed(9999));
    }

    #[test]
    fn test_empty_ports_allows_all() {
        let mut rules = test_rules();
        rules.allowed_ports = vec![];
        assert!(rules.is_port_allowed(9999));
    }
}
