//! Purpose: Shared stable hashing used by recall fingerprints and the workspace index.
//! Caller: utility::recall, utility::workspace_index.
//! Dependencies: None.
//! Main Functions: fnv1a64_hex.
//! Side Effects: None, pure function.

/// 64-bit FNV-1a rendered as 16 lowercase hex chars. The constants and output
/// format are load-bearing: recall fingerprints and workspace-index hashes are
/// persisted, so any change re-identifies every stored document. Both callers
/// previously carried byte-identical copies of this body.
pub(crate) fn fnv1a64_hex(content: &str) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
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
}
