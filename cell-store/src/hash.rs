use sha2::{Digest, Sha256};

/// Compute the SHA-256 digest of `data` and return it as `"sha256-<hex>"`.
pub fn sha256_digest(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("sha256-{:x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_digest_deterministic() {
        let d1 = sha256_digest(b"hello");
        let d2 = sha256_digest(b"hello");
        assert_eq!(d1, d2);
    }

    #[test]
    fn test_digest_prefix() {
        let d = sha256_digest(b"test");
        assert!(d.starts_with("sha256-"));
    }

    #[test]
    fn test_different_inputs() {
        let d1 = sha256_digest(b"hello");
        let d2 = sha256_digest(b"world");
        assert_ne!(d1, d2);
    }
}
