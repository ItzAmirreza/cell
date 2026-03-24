use sha2::{Digest, Sha256};

/// Compute the SHA-256 digest of `data` and return it as `"sha256-{hex}"`.
pub fn sha256_digest(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("sha256-{:x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_deterministic() {
        let a = sha256_digest(b"hello world");
        let b = sha256_digest(b"hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn digest_has_prefix() {
        let d = sha256_digest(b"test");
        assert!(d.starts_with("sha256-"), "digest should start with sha256- prefix");
    }

    #[test]
    fn different_data_different_digest() {
        let a = sha256_digest(b"aaa");
        let b = sha256_digest(b"bbb");
        assert_ne!(a, b);
    }

    #[test]
    fn known_value() {
        // SHA-256 of the empty byte slice is a well-known constant.
        let d = sha256_digest(b"");
        assert_eq!(
            d,
            "sha256-e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
