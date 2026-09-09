//! Purpose: Shared stable hashing used by recall fingerprints and the workspace index.
//! Caller: utility::recall, utility::workspace_index.
//! Dependencies: None.
//! Main Functions: fnv1a64_hex, sha256_hex.
//! Side Effects: None, pure function.

/// 64-bit FNV-1a rendered as 16 lowercase hex chars. The constants and output
/// format are load-bearing: recall fingerprints and workspace-index hashes are
/// persisted, so any change re-identifies every stored document. Both callers
/// previously carried byte-identical copies of this body.
pub(crate) fn fnv1a64_hex(content: &str) -> String {
    fnv1a64_bytes_hex(content.as_bytes())
}

/// Byte-oriented FNV-1a for binary artifacts and local dedupe. It is not a
/// cryptographic integrity primitive.
pub(crate) fn fnv1a64_bytes_hex(content: &[u8]) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in content {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

/// Cryptographic SHA-256 for persistent integrity claims and authenticated
/// opaque cursors. FNV remains available for cheap local dedupe/fingerprints;
/// callers must use this helper when tamper detection is part of the contract.
pub(crate) fn sha256_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(content);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_hex_matches_reference_vectors() {
        // Standard FNV-1a 64 test vectors.
        assert_eq!(fnv1a64_hex(""), "cbf29ce484222325");
        assert_eq!(fnv1a64_hex("a"), "af63dc4c8601ec8c");
        assert_eq!(fnv1a64_hex("foobar"), "85944171f73967e8");
    }

    #[test]
    fn sha256_hex_matches_reference_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
